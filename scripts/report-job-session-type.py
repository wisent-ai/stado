#!/usr/bin/env python3
"""Say which launchd session the reauth jobs run in, and what that session has.

A headed browser is a client of the WindowServer. Whether it can exist is
decided by the launchd session its process belongs to: `Aqua` is a logged-in
graphical session and has one, `Background` is a login-less session and does
not. The job plists carry that choice in `LimitLoadToSessionType`, and the
running process reports it as `launchctl managername`.

This prints, for every reauth job: where its plist lives, the session type it
asks for, the domain it is loaded in, and whether the graphical services exist
in that domain.
"""

import os
import pathlib
import plistlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LABELS = ("com.wisent.codex-reauth", "com.wisent.claude-reauth", "com.wisent.kimi-reauth")
ROOTS = (
    HOME / "Library" / "LaunchAgents",
    pathlib.Path("/Library/LaunchAgents"),
    pathlib.Path("/Library/LaunchDaemons"),
)
GRAPHICAL = ("com.apple.WindowServer", "com.apple.windowserver.active")


def plist_for(label):
    for root in ROOTS:
        candidate = root / f"{label}.plist"
        if candidate.is_file():
            return candidate
    return NONE


def loaded_domain(label):
    for domain in (f"gui/{os.getuid()}", f"user/{os.getuid()}", "system"):
        proc = subprocess.run(
            ["/bin/launchctl", "print", f"{domain}/{label}"], capture_output=True, text=True, check=False
        )
        if proc.returncode == ZERO:
            state = [line.strip() for line in proc.stdout.splitlines() if line.strip().startswith("state =")]
            return domain, (state[0] if state else "state unknown")
    return NONE, "not loaded in any domain"


def main():
    print(f"caller session {subprocess.run(['/bin/launchctl', 'managername'], capture_output=True, text=True).stdout.strip()}")
    for label in LABELS:
        path = plist_for(label)
        print(f"== {label}")
        if not path:
            print("   no plist found")
            continue
        document = plistlib.loads(path.read_bytes())
        print(f"   plist        {path}")
        print(f"   sessiontype  {document.get('LimitLoadToSessionType', '(unset -> Aqua for agents)')}")
        print(f"   interval     {document.get('StartInterval', '(none)')}")
        domain, state = loaded_domain(label)
        print(f"   loaded in    {domain or '(nowhere)'} {state}")
    print("== graphical services in this domain")
    for service in GRAPHICAL:
        for domain in (f"gui/{os.getuid()}", "system"):
            proc = subprocess.run(
                ["/bin/launchctl", "print", f"{domain}/{service}"], capture_output=True, text=True, check=False
            )
            answer = (proc.stdout or proc.stderr).strip().splitlines()[:1]
            print(f"   {domain}/{service:34} {answer[0][: len('a' * 50)] if answer else '(silent)'}")
    return NONE


sys.exit(main())
