#!/usr/bin/env python3
"""Print what each reauth unit on this host actually runs, and where it is loaded.

Placement arguments have been settled from labels -- "the claude job", "the
browser job" -- and a label is not a program. The unit that carries the label
`com.wisent.claude-reauth` runs `trajectories/claude/reauth.mjs`, whose failure
path spawns a headed browser login, and no amount of reading the label says so.

Read-only: it prints every unit file that carries one of these labels, the
program it runs, the session type it declares, and the launchd state in both the
system and graphical domains, so the installer's placement can be checked
against the machine rather than against memory.
"""

import os
import pathlib
import plistlib
import re
import subprocess
import sys

NONE = None
ZERO = len([])
FIRST = len(["first"])
HOME = pathlib.Path(os.path.expanduser("~"))
AGENTS = HOME / "Library" / "LaunchAgents"
DAEMONS = pathlib.Path("/Library/LaunchDaemons")
UID = os.getuid()
LABELS = ("com.wisent.codex-reauth", "com.wisent.claude-reauth", "com.wisent.kimi-reauth")
SUFFIXES = ("", ".awaiting-graphical-session", ".superseded-by-system-daemon")


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def state(domain, label):
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"{domain}/{label}") if domain == "system" \
        else run("/bin/launchctl", "print", f"{domain}/{label}")
    if not text.strip().startswith(f"{domain}/{label}"):
        return "absent"
    found = re.search(r"^\s*state = (.+)$", text, re.MULTILINE)
    pid = re.search(r"^\s*pid = (\d+)$", text, re.MULTILINE)
    return f"{found.group(FIRST).strip() if found else 'loaded'} pid {pid.group(FIRST) if pid else '-'}"


def main():
    print(f"session     {run('/bin/launchctl', 'managername').strip()}")
    print(f"gui/{UID}    {'present' if run('/bin/launchctl', 'print', f'gui/{UID}').strip().startswith(f'gui/{UID}') else 'absent'}")
    for label in LABELS:
        print(f"== {label}")
        print(f"   system   {state('system', label)}")
        print(f"   gui      {state(f'gui/{UID}', label)}")
        found = ZERO
        for directory in (DAEMONS, AGENTS):
            for suffix in SUFFIXES:
                path = directory / f"{label}.plist{suffix}"
                if not path.is_file():
                    continue
                found += FIRST
                try:
                    document = plistlib.loads(path.read_bytes())
                except (OSError, ValueError) as problem:
                    print(f"   file     {path} (unreadable: {problem})")
                    continue
                program = " ".join(str(item) for item in document.get("ProgramArguments", []))
                print(f"   file     {path}")
                print(f"     runs   {program or document.get('Program', '(nothing declared)')}")
                print(
                    f"     shape  session {document.get('LimitLoadToSessionType', '(none)')}  "
                    f"user {document.get('UserName', '(inherits)')}  "
                    f"interval {document.get('StartInterval', document.get('StartCalendarInterval', '(none)'))}"
                )
        if not found:
            print("   file     no unit file carries this label on this host")
    return NONE


sys.exit(main())
