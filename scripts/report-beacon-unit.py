#!/usr/bin/env python3
"""Say where this host's beacon actually runs from, and how often.

A repair that assumes `~/Library/LaunchAgents` refuses on a host whose always-on
units live in the system domain. Print every candidate that exists, its cadence,
and the domain launchd answers for, so the fix addresses the unit in force
rather than the one a laptop happens to use.
"""

import os
import pathlib
import plistlib
import subprocess
import sys

NONE = None
LABEL = os.environ.get("BEACON_LABEL", "com.wisent.host-health-beacon")
HOME = pathlib.Path(os.path.expanduser("~"))
CANDIDATES = (
    HOME / "Library" / "LaunchAgents" / f"{LABEL}.plist",
    pathlib.Path("/Library/LaunchAgents") / f"{LABEL}.plist",
    pathlib.Path("/Library/LaunchDaemons") / f"{LABEL}.plist",
)


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return (proc.stdout + proc.stderr).strip()


def main():
    for path in CANDIDATES:
        if not path.is_file():
            print(f"absent     {path}")
            continue
        try:
            document = plistlib.loads(path.read_bytes())
        except Exception as problem:  # unreadable is a finding, not a crash
            print(f"unreadable {path}: {problem}")
            continue
        environment = document.get("EnvironmentVariables", {}) or {}
        print(f"present    {path}")
        print(f"  interval {document.get('StartInterval')}")
        print(f"  api      {environment.get('STADO_HOST_HEALTH_API_URL', '(unset)')}")
        print(f"  writable {os.access(path, os.W_OK)}")
    for domain in (f"gui/{os.getuid()}", "system"):
        state = run("/bin/launchctl", "print", f"{domain}/{LABEL}")
        first = [line.strip() for line in state.splitlines() if "state = " in line]
        print(f"domain     {domain}: {first[0] if first else 'no such unit'}")
    return NONE


sys.exit(main())
