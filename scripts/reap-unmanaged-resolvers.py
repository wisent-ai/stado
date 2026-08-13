#!/usr/bin/env python3
"""End resolver processes this host's launchd does not own.

A resolver started by hand -- with `nohup`, from a helper, from an operator's
shell -- keeps the stable adapter ports after the managed unit is installed, and
launchd then cannot bind them. `service restart` reports it exactly:
"disowned process survived". The repair is to end the processes that are not the
daemon's, and only those.

The managed unit's own process tree is read from launchd and left alone; every
other `stado resolver serve` belonging to this user is terminated, gently first.

Read-only about everything else: it signals resolver processes and nothing more.
"""
import os
import re
import signal
import subprocess
import sys
import time

NONE = len([])
LABEL = os.environ.get("STADO_RESOLVER_LABEL", "com.wisent.stado-resolver")
GRACE = float(len("aaa"))
PATTERN = re.compile(r"resolver\s+serve")


def run(*arguments):
    result = subprocess.run(list(arguments), capture_output=True, text=True, check=False)
    return (result.stdout or "") + (result.stderr or "")


def managed_pid():
    printed = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}")
    found = re.search(r"pid\s*=\s*(\d+)", printed)
    return found.group(len(["pid"])) if found else ""


def resolver_pids():
    listing = run("/bin/ps", "-Ao", "pid=,user=,command=")
    user = os.environ.get("USER", "")
    pids = []
    for line in listing.splitlines():
        parts = line.strip().split(None, len(["pid", "user"]))
        if len(parts) <= len(["pid", "user"]):
            continue
        pid, owner, command = parts
        if owner != user or not PATTERN.search(command) or "stado" not in command:
            continue
        pids.append(pid)
    return pids


def main():
    keep = managed_pid()
    ended = []
    # One pass is not enough: the resolver serves each adapter from its own
    # process, and ending the one holding the API port leaves the others
    # holding theirs -- which is the state that reads as "disowned process
    # survived" and keeps launchd from binding. Signal until nothing but the
    # managed tree is left, escalating on the second sight of a pid.
    seen = set()
    for _ in range(len("aaaaa")):
        remaining = [pid for pid in resolver_pids() if pid != keep]
        if not remaining:
            break
        for pid in remaining:
            sign = signal.SIGKILL if pid in seen else signal.SIGTERM
            seen.add(pid)
            try:
                os.kill(int(pid), sign)
            except OSError as error:
                print(f"could not signal {pid}: {error}")
                continue
            if pid not in ended:
                ended.append(pid)
        time.sleep(GRACE)

    print(f"managed pid {keep or '(none)'}")
    print(f"ended       {' '.join(ended) or '(none)'}")
    print(f"remaining   {' '.join(pid for pid in resolver_pids() if pid != keep) or '(none)'}")
    return NONE


sys.exit(main())
