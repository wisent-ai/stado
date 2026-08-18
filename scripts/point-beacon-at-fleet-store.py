#!/usr/bin/env python3
"""Publish the host beacon where every other signal publishes.

This host's unit pinned `STADO_HOST_HEALTH_API_URL` to `http://127.0.0.1:8765`,
its own local health API, so its beacon only reached the fleet while a reverse
SSH tunnel to the authority happened to be alive. That tunnel dies on every
tailnet hiccup, and then `registry doctor` reports a stale beacon plus half a
dozen "missing plist" rows for services that are in fact running -- an outage of
information, not of hosts.

The beacon script means to default to the configured fleet store, but that
derivation runs under launchd's environment and came back empty here, which
fails the publish outright ("STADO_HOST_HEALTH_API_URL is required"). So the
unit carries the value the configuration already states, read from
`storage.stado.url`, and nothing is left to derive at run time. Keeps a
timestamped copy of the unit and restarts it.
"""

import datetime
import json
import os
import pathlib
import plistlib
import shutil
import subprocess
import sys
import time

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = os.environ.get("BEACON_LABEL", "com.wisent.host-health-beacon")
UNIT = HOME / "Library" / "LaunchAgents" / f"{LABEL}.plist"
CONFIG = HOME / ".config" / "stado" / "config.json"
KEY = "STADO_HOST_HEALTH_API_URL"
SETTLE = 20


def configured_store():
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    url = ((document.get("storage") or {}).get("stado") or {}).get("url") or ""
    if not url:
        raise SystemExit(f"{CONFIG} names no storage.stado.url to publish to")
    return url


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def main():
    if not UNIT.is_file():
        raise SystemExit(f"no beacon unit at {UNIT}")
    wanted = configured_store()
    document = plistlib.loads(UNIT.read_bytes())
    environment = dict(document.get("EnvironmentVariables", {}))
    before = environment.get(KEY, "(unset)")
    print(f"unit       {UNIT}")
    print(f"before     {KEY}={before}")
    print(f"wanted     {KEY}={wanted}")
    if before == wanted:
        print("settled    the beacon already publishes to the configured store")
        return NONE
    stamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%SZ")
    shutil.copy2(UNIT, UNIT.with_name(f"{UNIT.name}.before-{stamp}"))
    environment[KEY] = wanted
    document["EnvironmentVariables"] = environment
    with UNIT.open("wb") as handle:
        plistlib.dump(document, handle)
    run("/bin/launchctl", "bootout", f"gui/{os.getuid()}/{LABEL}")
    time.sleep(len("ab"))
    run("/bin/launchctl", "bootstrap", f"gui/{os.getuid()}", str(UNIT))
    time.sleep(len("ab"))
    run("/bin/launchctl", "kickstart", f"gui/{os.getuid()}/{LABEL}")
    time.sleep(SETTLE)
    print(f"after      the beacon posts to {wanted}")
    return NONE


sys.exit(main())
