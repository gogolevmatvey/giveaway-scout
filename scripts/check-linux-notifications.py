#!/usr/bin/env python3
"""Exercise the real Rust notification adapter on a private, fake D-Bus desktop.

No application window, browser, HTTP request, user database or desktop toast is used.
Requires Linux, dbus-run-session and the system Python's PyGObject (python3-gi).
"""

import json
import os
from pathlib import Path
import subprocess
import sys
from xml.sax.saxutils import escape


PROJECT = Path(__file__).resolve().parent.parent
TAURI = PROJECT / "src" / "GiveawayScout.Desktop" / "src-tauri"
TEST = "notifications::tests::linux_dbus_round_trip"
CASES = (
    "default", "open", "dismissed", "expired", "unknown", "no-actions",
    "plain-body", "sound-on", "rejected", "capabilities-error",
)
TITLE = "<b>Title</b> & example"
BODY = 'Source: <a href="https://untrusted.invalid/">Offer</a> & \'text\''
INTERFACE = "org.freedesktop.Notifications"
OBJECT = "/org/freedesktop/Notifications"

XML = """
<node><interface name="org.freedesktop.Notifications">
  <method name="GetCapabilities"><arg type="as" direction="out"/></method>
  <method name="GetServerInformation">
    <arg type="s" direction="out"/><arg type="s" direction="out"/>
    <arg type="s" direction="out"/><arg type="s" direction="out"/>
  </method>
  <method name="Notify">
    <arg type="s" direction="in"/><arg type="u" direction="in"/>
    <arg type="s" direction="in"/><arg type="s" direction="in"/>
    <arg type="s" direction="in"/><arg type="as" direction="in"/>
    <arg type="a{sv}" direction="in"/><arg type="i" direction="in"/>
    <arg type="u" direction="out"/>
  </method>
  <method name="CloseNotification"><arg type="u" direction="in"/></method>
  <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
  <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
</interface></node>
"""


def build_test_binary():
    result = subprocess.run(
        ["cargo", "test", "--locked", "--workspace", "--no-run", "--message-format=json"],
        cwd=TAURI, check=True, stdout=subprocess.PIPE, text=True,
    )
    for line in result.stdout.splitlines():
        artifact = json.loads(line)
        if (artifact.get("reason") == "compiler-artifact"
                and artifact.get("target", {}).get("name") == "giveaway_scout_lib"
                and artifact.get("executable")):
            return artifact["executable"]
    raise RuntimeError("Cargo did not report the Giveaway Scout library test binary")


def private_session(binary):
    original_bus = os.environ.get("GIVEAWAY_SCOUT_ORIGINAL_BUS")
    if original_bus is None or os.environ.get("DBUS_SESSION_BUS_ADDRESS", "") == original_bus:
        raise RuntimeError("Refusing to use the original desktop bus; run without --private")
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib

    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    owned = connection.call_sync(
        "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
        "RequestName", GLib.Variant("(su)", (INTERFACE, 4)),
        GLib.VariantType.new("(u)"), Gio.DBusCallFlags.NONE, 5000, None,
    ).unpack()[0]
    if owned != 1:
        raise RuntimeError("Private notification service name was not acquired")

    class Desktop:
        case = ""
        generation = 0
        notifications = 0
        errors = None
        child = None

        def emit_response(self, identifier, generation):
            if generation != self.generation or self.child.poll() is not None:
                return False
            if self.case in ("dismissed", "expired"):
                reason = 2 if self.case == "dismissed" else 1
                name, payload = "NotificationClosed", GLib.Variant("(uu)", (identifier, reason))
            else:
                action = "unexpected" if self.case == "unknown" else "open" if self.case == "open" else "default"
                name, payload = "ActionInvoked", GLib.Variant("(us)", (identifier, action))
            connection.emit_signal(None, OBJECT, INTERFACE, name, payload)
            # Retry until the client exits: listener setup need not win a timing race.
            return True

        def method(self, _connection, _sender, _path, _interface, method, parameters, invocation):
            try:
                if method == "GetCapabilities":
                    if self.case == "capabilities-error":
                        invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", "test capabilities failure")
                        return
                    capabilities = []
                    if self.case != "no-actions":
                        capabilities.append("actions")
                    if self.case != "plain-body":
                        capabilities.append("body-markup")
                    invocation.return_value(GLib.Variant("(as)", (capabilities,)))
                elif method == "GetServerInformation":
                    invocation.return_value(GLib.Variant("(ssss)", ("Scout fake desktop", "Test", "1", "1.3")))
                elif method == "Notify":
                    self.notifications += 1
                    app, replaces, _icon, title, body, actions, hints, timeout = parameters.unpack()
                    assert app == "Giveaway Scout", app
                    assert title == TITLE, title
                    expected_body = BODY if self.case == "plain-body" else escape(BODY, {'"': "&quot;", "'": "&apos;"})
                    assert body == expected_body, body
                    expected_actions = [] if self.case in ("no-actions", "capabilities-error") else ["default", "Открыть", "open", "Открыть"]
                    assert actions == expected_actions, actions
                    assert hints.get("desktop-entry") == "Giveaway Scout", hints
                    assert hints.get("suppress-sound") is (self.case != "sound-on"), hints
                    assert ("sound-name" in hints) is (self.case == "sound-on"), hints
                    if self.case == "sound-on":
                        assert hints["sound-name"] == "message-new-instant", hints
                    assert replaces == 0 and timeout == -1, (replaces, timeout)
                    if self.case == "rejected":
                        invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", "test rejection")
                        return
                    identifier = self.generation
                    invocation.return_value(GLib.Variant("(u)", (identifier,)))
                    if actions:
                        GLib.timeout_add(100, self.emit_response, identifier, self.generation)
                elif method == "CloseNotification":
                    invocation.return_value(None)
                else:
                    raise AssertionError("Unexpected D-Bus method: " + method)
            except Exception as error:
                self.errors.append(repr(error))
                invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", "contract violation: " + repr(error))

    desktop = Desktop()
    info = Gio.DBusNodeInfo.new_for_xml(XML)
    registration = connection.register_object(OBJECT, info.interfaces[0], desktop.method, None, None)
    try:
        for generation, case in enumerate(CASES, start=1):
            desktop.case, desktop.generation = case, generation
            desktop.notifications, desktop.errors = 0, []
            environment = {**os.environ, "GIVEAWAY_SCOUT_NOTIFICATION_SMOKE": case}
            desktop.child = subprocess.Popen(
                [binary, "--ignored", "--exact", TEST, "--nocapture"], env=environment,
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            )
            loop = GLib.MainLoop()
            timed_out = False

            def poll_child():
                if desktop.child.poll() is not None:
                    loop.quit()
                    return False
                return True

            def stop_child():
                nonlocal timed_out
                timed_out = True
                desktop.child.kill()
                return False

            GLib.timeout_add(25, poll_child)
            watchdog = GLib.timeout_add_seconds(15, stop_child)
            loop.run()
            if not timed_out:
                GLib.source_remove(watchdog)
            output, _ = desktop.child.communicate()
            if timed_out or desktop.child.returncode or desktop.errors or desktop.notifications != 1:
                raise RuntimeError(f"{case}: D-Bus test failed\n{output}\n{desktop.errors}")
            print(f"PASS: {case}", flush=True)
    finally:
        if desktop.child is not None and desktop.child.poll() is None:
            desktop.child.kill()
            desktop.child.communicate()
        connection.unregister_object(registration)
    print("10 Linux notification scenarios passed on a private bus; no user desktop notifications were sent.")


if __name__ == "__main__":
    if sys.platform != "linux":
        raise SystemExit("This check requires Linux. Use the native Windows checks separately.")
    if len(sys.argv) == 3 and sys.argv[1] == "--private":
        private_session(sys.argv[2])
    elif len(sys.argv) == 1:
        binary = build_test_binary()
        subprocess.run(
            ["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), "--private", binary],
            env={**os.environ, "GIVEAWAY_SCOUT_ORIGINAL_BUS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")},
            check=True,
        )
    else:
        raise SystemExit("Usage: /usr/bin/python3 scripts/check-linux-notifications.py")
