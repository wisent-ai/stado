#!/usr/bin/env python3
"""Publish the beacon more often than a reader is willing to wait for it.

Readers treat a host health document older than 180 seconds as stale. A launchd
`StartInterval` is a request, not a promise -- the timer coalesces, and on a busy
laptop a 60 second interval was observed leaving five minute gaps, which reads as
a dead host and pulls a `missing-plist` row for every service the registry
declares for it.

So the interval is 15 seconds: launchd honours a 15 second request within its 10
the window mean a coalesced or lost cycle is not a stale host. Keeps a timestamped
copy of the unit and reloads it. Idempotent.
"""

import datetime
import os
import pathlib
import plistlib
import shutil
import subprocess
import sys
import time

NONE = None
WANTED = 15
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = os.environ.get("BEACON_LABEL", "com.wisent.host-health-beacon")
UNIT = HOME / "Library" / "LaunchAgents" / f"{LABEL}.plist"


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def main():
    if not UNIT.is_file():
        raise SystemExit(f"no beacon unit at {UNIT}")
    document = plistlib.loads(UNIT.read_bytes())
    before = document.get("StartInterval")
    print(f"unit       {UNIT}")
    print(f"before     StartInterval={before}")
    if before == WANTED:
        print(f"settled    already publishes every {WANTED}s")
        return NONE
    stamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%SZ")
    shutil.copy2(UNIT, UNIT.with_name(f"{UNIT.name}.before-{stamp}"))
    document["StartInterval"] = WANTED
    with UNIT.open("wb") as handle:
        plistlib.dump(document, handle)
    run("/bin/launchctl", "bootout", f"gui/{os.getuid()}/{LABEL}")
    time.sleep(len("ab"))
    run("/bin/launchctl", "bootstrap", f"gui/{os.getuid()}", str(UNIT))
    run("/bin/launchctl", "kickstart", f"gui/{os.getuid()}/{LABEL}")
    time.sleep(len("abcdefghij"))
    print(f"after      StartInterval={plistlib.loads(UNIT.read_bytes()).get('StartInterval')}")
    return NONE


sys.exit(main())
