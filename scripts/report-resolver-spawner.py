#!/usr/bin/env python3
"""Say what starts the resolver when launchd is not the one doing it.

A resolver keeps reappearing on the API port seconds after being ended, while
`launchctl print` shows the managed job loaded and not running. Something else
is starting it, and until that is named every repair is undone by the next
tick. This prints the live resolver processes with their parents, the parents'
own commands, and the loaded launchd jobs whose program is the stado binary.

Read-only: it inspects processes and launchd records.
"""

import os
import pathlib
import re
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = "com.wisent.stado-resolver"


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def processes():
    rows = []
    for line in run("/bin/ps", "-Ao", "pid=,ppid=,lstart=,command=").splitlines():
        if "resolver" in line and "stado" in line and "report-resolver" not in line:
            fields = line.split(None, len(["pid", "ppid"]))
            if len(fields) > len(["pid", "ppid"]):
                rows.append((fields[ZERO], fields[len(["pid"])], fields[-1]))
    return rows


def command_of(pid):
    text = run("/bin/ps", "-p", str(pid), "-o", "command=").strip()
    return text.splitlines()[ZERO] if text else "(gone)"


def main():
    print(f"managed job:")
    printed = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}")
    for key in ("state", "pid", "last exit code", "runs"):
        found = re.search(rf"^\s*{re.escape(key)} = (.+)$", printed, re.MULTILINE)
        print(f"  {key:<15} {found.group(len(['v'])).strip() if found else '(absent)'}")

    print("live resolver processes:")
    for pid, ppid, command in processes():
        print(f"  pid {pid:<7} ppid {ppid:<7} {command[: len('a' * 90)]}")
        print(f"    parent: {command_of(ppid)[: len('a' * 90)]}")

    print("launchd jobs running the stado binary:")
    for line in run("/bin/launchctl", "list").splitlines():
        parts = line.split("\t")
        if len(parts) == len(["pid", "status", "label"]) and "wisent" in parts[-1]:
            print(f"  {parts[-1]:<52} pid {parts[ZERO]}")
    return NONE


sys.exit(main())
