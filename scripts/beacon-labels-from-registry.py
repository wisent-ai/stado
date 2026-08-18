#!/usr/bin/env python3
"""Report the services the registry declares, not a list frozen beside the beacon.

The beacon reads `WC_HEALTH_UNITS` if it is set and only then falls back to the
registry. Both hosts carried a hand-written copy of that list -- the laptop in
its launchd unit, the authority in its launcher -- and both had drifted: the
laptop published a units map holding only the beacon itself, so `registry doctor`
reported a missing plist for six services that were installed and loaded, and the
authority reported seven of the fourteen it declares.

`stado-rs/scripts/register-beacon-unit-macos.sh` already retired this key in the
unit for the same reason, then kept editing the launcher's copy by hand. The list
has one source of truth, the registry, and any copy silently wins over it. This
removes both copies, keeps a timestamped backup of whatever it edits, and refuses
to leave a launcher that would not parse.
"""

import datetime
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import sys
import time

NONE = None
KEY = "WC_HEALTH_UNITS"
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = os.environ.get("BEACON_LABEL", "com.wisent.host-health-beacon")
UNIT = HOME / "Library" / "LaunchAgents" / f"{LABEL}.plist"
LAUNCHER = HOME / ".stado" / "bin" / "host-health-beacon-launcher"
SETTLE = 25


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return (proc.stdout + proc.stderr).strip()


def from_plist():
    """The laptop shape: the launchd unit carries the environment."""
    if not UNIT.is_file():
        return "no unit here"
    document = plistlib.loads(UNIT.read_bytes())
    environment = dict(document.get("EnvironmentVariables") or {})
    frozen = environment.get(KEY)
    if frozen is NONE:
        return "already derived"
    stamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%SZ")
    shutil.copy2(UNIT, UNIT.with_name(f"{UNIT.name}.before-{stamp}"))
    environment.pop(KEY)
    document["EnvironmentVariables"] = environment
    with UNIT.open("wb") as handle:
        plistlib.dump(document, handle)
    run("/bin/launchctl", "bootout", f"gui/{os.getuid()}/{LABEL}")
    time.sleep(len("ab"))
    run("/bin/launchctl", "bootstrap", f"gui/{os.getuid()}", str(UNIT))
    run("/bin/launchctl", "kickstart", f"gui/{os.getuid()}/{LABEL}")
    time.sleep(SETTLE)
    return f"removed from unit (was {frozen[:44]}...)"


def from_launcher():
    """The authority shape: a KeepAlive loop exports the list before publishing.

    The line is emptied, not deleted. The launcher runs under `set -u` and names
    `$WC_HEALTH_UNITS` again when it records its runtime, so removing the export
    made it die before its loop and launchd respawned it every few seconds --
    publishing often enough to look alive while never staying up. An empty value
    falls through to the registry derivation and keeps the later reference bound.
    """
    if not LAUNCHER.is_file():
        return "no launcher here"
    original = LAUNCHER.read_text(encoding="utf-8")
    lines = original.splitlines(keepends=True)
    kept = [line for line in lines if not re.match(rf'^export {KEY}=', line)]
    first = next((i for i, line in enumerate(kept) if KEY in line), NONE)
    anchor = next((i + 1 for i, line in enumerate(kept) if line.startswith("set -")), 1)
    where = anchor if first is NONE else min(anchor, first)
    wanted = f'export {KEY}=""\n'
    if kept[where - 1 : where] == [wanted] or (
        where < len(kept) and kept[where] == wanted
    ):
        return "already derived"
    kept.insert(where, wanted)
    body = "".join(kept)
    if body == original:
        return "already derived"
    LAUNCHER.with_name(LAUNCHER.name + ".before-labels").write_text(original, encoding="utf-8")
    LAUNCHER.write_text(body, encoding="utf-8")
    check = subprocess.run(["/bin/sh", "-n", str(LAUNCHER)], capture_output=True,
                           text=True, check=False)
    if check.returncode != 0:
        LAUNCHER.write_text(original, encoding="utf-8")
        raise SystemExit(f"edit refused, launcher restored: {check.stderr.strip()}")
    return f"empty export placed at line {where + 1}, above every mention; reload the unit"


def main():
    print(f"plist      {from_plist()}")
    print(f"launcher   {from_launcher()}")
    print("after      labels come from the registry this host is declared in")
    return NONE


sys.exit(main())
