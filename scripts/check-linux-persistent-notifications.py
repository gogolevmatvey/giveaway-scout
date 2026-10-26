#!/usr/bin/env python3
"""Verify persistent sender -> saved payload -> cold D-Bus receiver on a private bus.

No real toast, sound, application window, browser, HTTP request or user database.
The desktop is a fake; service auto-start and Rust sender/receiver are real.
"""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import time

PROJECT = Path(__file__).resolve().parent.parent
TAURI = PROJECT / "src" / "GiveawayScout.Desktop" / "src-tauri"
APP = "com.giveawayscout.desktop.activationtest"
OBJECT = "/com/giveawayscout/desktop/activationtest"
SENDER = "linux_notifications::tests::persistent_dbus_fixture"
RECEIVER = "linux_activation::tests::dbus_activation_fixture"
POLICY = "linux_notifications::tests::sound_policy_fixture"
ID = "550e8400-e29b-41d4-a716-446655440000"
GTK = "org.gtk.Notifications"
XML = """<node><interface name="org.gtk.Notifications">
<method name="AddNotification"><arg type="s" direction="in"/>
<arg type="s" direction="in"/><arg type="a{sv}" direction="in"/></method>
</interface></node>"""
LOCK_XML = """<node><interface name="org.gnome.ScreenSaver">
<method name="GetActive"><arg type="b" direction="out"/></method>
</interface></node>"""


def build_test_binary():
    build = subprocess.run(
        ["cargo", "test", "--locked", "--workspace", "--no-run", "--message-format=json"],
        cwd=TAURI, check=True, text=True, stdout=subprocess.PIPE,
    )
    for line in build.stdout.splitlines():
        artifact = json.loads(line)
        if (artifact.get("reason") == "compiler-artifact"
                and artifact.get("target", {}).get("name") == "giveaway_scout_lib"
                and artifact.get("executable")):
            return artifact["executable"]
    raise RuntimeError("Cargo did not report the library test executable")


def private_session(binary):
    original = os.environ.get("GIVEAWAY_SCOUT_ORIGINAL_BUS")
    if original is None or os.environ.get("DBUS_SESSION_BUS_ADDRESS", "") == original:
        raise RuntimeError("Refusing to use the original desktop bus")
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    context = GLib.MainContext.default()
    log = Path(os.environ["GIVEAWAY_SCOUT_ACTIVATION_LOG"])

    def wait_for(predicate, message, timeout=12):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            while context.pending():
                context.iteration(False)
            if predicate():
                return
            time.sleep(0.01)
        raise AssertionError(message)

    def bus_call(method, name):
        return connection.call_sync(
            "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
            method, GLib.Variant("(s)", (name,)), None, Gio.DBusCallFlags.NONE, 5000, None,
        ).unpack()[0]

    def own(name):
        result = connection.call_sync(
            "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
            "RequestName", GLib.Variant("(su)", (name, 4)), None,
            Gio.DBusCallFlags.NONE, 5000, None,
        ).unpack()[0]
        assert result == 1, (name, result)

    def run_fixture(test, extra):
        child = subprocess.Popen(
            [binary, "--ignored", "--exact", test, "--nocapture"],
            env={**os.environ, **extra}, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        try:
            wait_for(lambda: child.poll() is not None, "Rust fixture timed out")
            output = child.communicate(timeout=2)[0]
            assert child.returncode == 0, output
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()

    saved, errors, senders, calls, pending = {}, [], [], [], []
    current_case = "absent"
    lock_case = "unlocked"

    def add_notification(_connection, sender, _path, _interface, method, parameters, invocation):
        try:
            assert method == "AddNotification", method
            app, identifier, values = parameters.unpack()
            assert app == APP, app
            assert values["title"] == "<b>Title</b> & example", values
            assert values["body"] == "<a href='https://untrusted.invalid/'>Offer</a> & text", values
            assert values["priority"] == "normal", values
            kind, icon = values["icon"]
            assert kind == "bytes" and bytes(icon).startswith(b"\x89PNG\r\n\x1a\n"), kind
            button = values["buttons"][0]
            assert button["label"] == "Открыть"
            assert button["action"] == values["default-action"]
            if current_case == "show":
                assert identifier == "test" and values["default-action"] == "app.show"
                assert "target" not in button and "default-action-target" not in values
            else:
                assert identifier == ID and values["default-action"] == "app.open-notification"
                assert values["default-action-target"] == button["target"] == ID
            calls.append((app, identifier))
            senders.append(sender)
            if current_case == "rejected":
                invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", "test rejection")
            else:
                saved[(app, identifier)] = values
                if current_case == "timeout":
                    pending.append(invocation)
                else:
                    invocation.return_value(None)
        except Exception as error:
            errors.append(repr(error))
            invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", "contract violation")

    def screen_saver(_connection, _sender, _path, _interface, _method, _parameters, invocation):
        if lock_case == "error":
            invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", "test lock failure")
        else:
            invocation.return_value(GLib.Variant("(b)", (lock_case == "locked",)))

    run_fixture(SENDER, {"GIVEAWAY_SCOUT_PERSISTENT_CASE": "absent"})
    assert not calls
    print("PASS: missing persistent server selects fallback before sending", flush=True)
    own(GTK)
    gtk_info = Gio.DBusNodeInfo.new_for_xml(XML)
    lock_info = Gio.DBusNodeInfo.new_for_xml(LOCK_XML)
    gtk_registration = connection.register_object("/org/gtk/Notifications", gtk_info.interfaces[0], add_notification, None, None)
    own("org.gnome.ScreenSaver")
    lock_registration = connection.register_object("/org/gnome/ScreenSaver", lock_info.interfaces[0], screen_saver, None, None)
    try:
        for current_case in ("rejected", "timeout", "uuid", "repeat", "show"):
            run_fixture(SENDER, {"GIVEAWAY_SCOUT_PERSISTENT_CASE": current_case})
            assert not errors, errors
            assert not bus_call("NameHasOwner", senders[-1]), "sender process must exit before activation"
            for invocation in pending:
                invocation.return_value(None)
            pending.clear()
            print("PASS: acknowledged persistent send: " + current_case, flush=True)
        assert len(calls) == 6, calls
        assert len(saved) == 2, "same UUID must replace instead of duplicating"
        assert not bus_call("NameHasOwner", APP), "receiver must not be running yet"

        def activate(payload):
            action = payload["default-action"]
            parameters = [GLib.Variant("s", payload["default-action-target"])] if "default-action-target" in payload else []
            connection.call_sync(
                APP, OBJECT, "org.freedesktop.Application", "ActivateAction",
                GLib.Variant("(sava{sv})", (action.removeprefix("app."), parameters, {})),
                None, Gio.DBusCallFlags.NONE, 10000, None,
            )

        activate(saved[(APP, ID)])
        wait_for(lambda: log.exists() and ("notification:" + ID) in log.read_text(), "saved UUID did not cold-start receiver")
        owner = bus_call("GetNameOwner", APP)
        activate(saved[(APP, ID)])
        assert bus_call("GetNameOwner", APP) == owner, "warm click must reuse process"
        activate(saved[(APP, "test")])
        wait_for(lambda: not bus_call("NameHasOwner", APP), "receiver did not shut down after three actions")
        assert log.read_text().splitlines().count("notification:" + ID) == 2
        assert log.read_text().splitlines().count("show") == 1
        print("PASS: saved payload cold-starts real Rust receiver after sender exits; warm click and app.show", flush=True)

        for lock_case in ("unlocked", "locked", "error"):
            run_fixture(POLICY, {"GIVEAWAY_SCOUT_LOCK_CASE": lock_case})
            print("PASS: isolated sound policies and screen state: " + lock_case, flush=True)
    finally:
        connection.unregister_object(gtk_registration)
        connection.unregister_object(lock_registration)
    print("Persistent notification checks passed. No real toast, sound, browser or user database was used.")


if __name__ == "__main__":
    if sys.platform != "linux":
        raise SystemExit("This check requires Linux")
    if len(sys.argv) == 3 and sys.argv[1] == "--private":
        private_session(sys.argv[2])
    elif len(sys.argv) == 1:
        binary = build_test_binary()
        with tempfile.TemporaryDirectory(prefix="giveaway-scout-persistent-") as temporary:
            root = Path(temporary)
            services = root / "data" / "dbus-1" / "services"
            services.mkdir(parents=True)
            (root / "empty").mkdir()
            (services / (APP + ".service")).write_text(
                "[D-BUS Service]\nName=" + APP + "\nExec="
                + shlex.join([binary, "--ignored", "--exact", RECEIVER, "--nocapture"]) + "\n"
            )
            subprocess.run(
                ["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), "--private", binary],
                check=True, env={
                    **os.environ, "XDG_DATA_HOME": str(root / "data"), "XDG_DATA_DIRS": str(root / "empty"),
                    "XDG_CONFIG_HOME": str(root / "config"), "GSETTINGS_BACKEND": "memory",
                    "GSETTINGS_SCHEMA_DIR": "/usr/share/glib-2.0/schemas",
                    "GIVEAWAY_SCOUT_ACTIVATION_LOG": str(root / "activation.log"),
                    "GIVEAWAY_SCOUT_ORIGINAL_BUS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", ""),
                },
            )
    else:
        raise SystemExit("Usage: /usr/bin/python3 scripts/check-linux-persistent-notifications.py")
