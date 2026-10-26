#!/usr/bin/env python3
"""Test Gio cold/warm activation without installing the app or opening user data."""

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
APP_ID = "com.giveawayscout.desktop.activationtest"
OBJECT = "/com/giveawayscout/desktop/activationtest"
TEST = "linux_activation::tests::dbus_activation_fixture"
FIRST = "550e8400-e29b-41d4-a716-446655440000"
SECOND = "550e8400-e29b-41d4-a716-446655440001"


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


def private_session():
    original = os.environ.get("GIVEAWAY_SCOUT_ORIGINAL_BUS")
    if original is None or os.environ.get("DBUS_SESSION_BUS_ADDRESS", "") == original:
        raise RuntimeError("Refusing to use the original desktop bus")
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib

    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    log = Path(os.environ["GIVEAWAY_SCOUT_ACTIVATION_LOG"])

    def lines():
        return log.read_text().splitlines() if log.exists() else []

    def wait_for(predicate, message):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.025)
        raise AssertionError(f"{message}: {lines()}")

    def bus_call(method, argument):
        return connection.call_sync(
            "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
            method, GLib.Variant("(s)", (argument,)), None,
            Gio.DBusCallFlags.NONE, 10000, None,
        ).unpack()[0]

    def action(name, target):
        connection.call_sync(
            APP_ID, OBJECT, "org.freedesktop.Application", "ActivateAction",
            GLib.Variant("(sava{sv})", (name, [GLib.Variant("s", target)], {})),
            None, Gio.DBusCallFlags.NONE, 10000, None,
        )

    pids = []
    for generation in (1, 2):
        assert not bus_call("NameHasOwner", APP_ID), "the previous process must be closed"
        previous = lines()
        action("open-notification", FIRST)
        wait_for(lambda: len(lines()) == len(previous) + 2, "cold activation did not deliver the UUID")
        startup = lines()[-2:]
        ready = next(line for line in startup if line.startswith("ready:"))
        pids.append(int(ready.split(":")[1]))
        assert "notification:" + FIRST in startup, startup
        owner = bus_call("GetNameOwner", APP_ID)

        unchanged = lines()
        for value in ("https://untrusted.invalid/", "../settings.json", FIRST + "?url=other"):
            action("open-notification", value)
        try:
            action("unknown-action", FIRST)
        except GLib.Error as error:
            if not error.matches(Gio.dbus_error_quark(), Gio.DBusError.INVALID_ARGS):
                raise
        else:
            raise AssertionError("unknown actions must be rejected by D-Bus")
        time.sleep(0.1)
        assert lines() == unchanged, "invalid input must never reach the app callback"

        action("open-notification", SECOND.upper())
        wait_for(lambda: lines()[-1] == "notification:" + SECOND, "warm activation did not normalize the UUID")
        assert bus_call("GetNameOwner", APP_ID) == owner, "warm activation must reuse the process"
        assert len([line for line in lines() if line.startswith("ready:")]) == generation

        connection.call_sync(
            APP_ID, OBJECT, "org.freedesktop.Application", "Activate",
            GLib.Variant("(a{sv})", ({},)), None, Gio.DBusCallFlags.NONE, 10000, None,
        )
        wait_for(lambda: lines()[-1] == "show", "generic activation must show the app")
        wait_for(lambda: not bus_call("NameHasOwner", APP_ID), "fixture process did not shut down")
        print(f"PASS: cold/warm activation, invalid input and shutdown (generation {generation})", flush=True)
    assert pids[0] != pids[1], "activation after exit must start a new process"
    print("Linux D-Bus activation passed. No app window, browser, HTTP request or user database was used.")


if __name__ == "__main__":
    if sys.platform != "linux":
        raise SystemExit("This check requires Linux")
    if sys.argv[1:] == ["--private"]:
        private_session()
    elif len(sys.argv) == 1:
        binary = build_test_binary()
        with tempfile.TemporaryDirectory(prefix="giveaway-scout-activation-") as temporary:
            root = Path(temporary)
            services = root / "data" / "dbus-1" / "services"
            services.mkdir(parents=True)
            (root / "empty").mkdir()
            command = shlex.join([binary, "--ignored", "--exact", TEST, "--nocapture"])
            (services / (APP_ID + ".service")).write_text(
                f"[D-BUS Service]\nName={APP_ID}\nExec={command}\n"
            )
            subprocess.run(
                ["dbus-run-session", "--", sys.executable, str(Path(__file__).resolve()), "--private"],
                check=True,
                env={
                    **os.environ,
                    "XDG_DATA_HOME": str(root / "data"),
                    "XDG_DATA_DIRS": str(root / "empty"),
                    "GIVEAWAY_SCOUT_ACTIVATION_LOG": str(root / "activation.log"),
                    "GIVEAWAY_SCOUT_ORIGINAL_BUS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", ""),
                },
            )
    else:
        raise SystemExit("Usage: /usr/bin/python3 scripts/check-linux-activation.py")
