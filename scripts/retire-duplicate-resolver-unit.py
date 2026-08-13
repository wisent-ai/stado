#!/usr/bin/env python3
"""Leave exactly one launchd unit serving the resolver on this host.

This host already had `com.wisent.stado-resolver` when a second unit,
`com.wisent.always-on.stado-resolver`, was installed beside it. Both name the
same program and the same loopback ports, so whichever loses the race spends
its life failing with "Address already in use" while launchd keeps respawning
it -- and every repair aimed at one of them is undone by the other.

The older label is the fleet's; the newcomer goes. Its unit is booted out and
its file removed, then the surviving unit is loaded if it is not already, and
both records are printed so the outcome is read rather than assumed.
"""

import os
import pathlib
import re
import subprocess
import sys
import time

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
DUPLICATE = "com.wisent.always-on.stado-resolver"
KEEP = "com.wisent.stado-resolver"
DAEMONS = pathlib.Path("/Library/LaunchDaemons")


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def record(label):
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{label}")
    if not text.strip().startswith(f"system/{label}"):
        return "not loaded in the system domain"
    state = re.search(r"^\s*state = (.+)$", text, re.MULTILINE)
    pid = re.search(r"^\s*pid = (\d+)$", text, re.MULTILINE)
    active = re.search(r"^\s*active count = (\d+)$", text, re.MULTILINE)
    return (
        f"state {state.group(len(['v'])).strip() if state else '?'}, "
        f"pid {pid.group(len(['v'])) if pid else '(none)'}, "
        f"active {active.group(len(['v'])) if active else '?'}"
    )


def main():
    duplicate_plist = DAEMONS / f"{DUPLICATE}.plist"
    if duplicate_plist.is_file():
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{DUPLICATE}")
        run("/usr/bin/sudo", "-n", "/bin/rm", "-f", str(duplicate_plist))
        print(f"removed    {duplicate_plist}")
    else:
        print(f"absent     {duplicate_plist}")

    # Ending the processes has to happen after the duplicate is gone, or the
    # unit that is still scheduled simply takes the freed port again.
    for _ in range(len("aaa")):
        pids = [
            line.split()[ZERO]
            for line in run("/bin/ps", "-Ao", "pid=,command=").splitlines()
            if "resolver serve" in line and "stado" in line
        ]
        if not pids:
            break
        for pid in pids:
            run("/bin/kill", "-TERM", pid)
        time.sleep(len("aa"))

    keep_plist = DAEMONS / f"{KEEP}.plist"
    if keep_plist.is_file():
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "enable", f"system/{KEEP}")
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(keep_plist))
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "kickstart", f"system/{KEEP}")
    time.sleep(len("a" * 5))
    print(f"{DUPLICATE}: {record(DUPLICATE)}")
    print(f"{KEEP}: {record(KEEP)}")
    return NONE


sys.exit(main())
