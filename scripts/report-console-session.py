#!/usr/bin/env python3
"""Say whether this host has a console session a browser can run in.

Chromium on macOS needs the user's session: without one it reports
`NSNotificationCenter connection invalid` and `Encryption is not available`, and
this patched build then dies before it paints. That failure looks like a broken
browser and is a missing login.

Prints who is on the console, whether automatic login is configured, and whether
the user's launchd domain is reachable.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LOGIN_WINDOW = "/Library/Preferences/com.apple.loginwindow"


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return (proc.stdout + proc.stderr).strip()


def main():
    print(f"who         {run('/usr/bin/who') or '(nobody on any terminal)'}")
    print(f"console     {run('/usr/bin/stat', '-f', '%Su', '/dev/console')}")
    print(f"auto login  {run('/usr/bin/defaults', 'read', LOGIN_WINDOW, 'autoLoginUser') or '(not set)'}")
    print(f"uid         {os.getuid()}")
    for domain in (f"gui/{os.getuid()}", f"user/{os.getuid()}"):
        answer = run("/bin/launchctl", "print", domain)
        first = answer.splitlines()[:len(["l"])]
        print(f"domain {domain:<12} {first[ZERO][: len('a' * 90)] if first else '(no answer)'}")
    print(f"screen lock {run('/usr/bin/pmset', '-g', 'ps').splitlines()[:len(['l'])]}")
    return NONE


sys.exit(main())
