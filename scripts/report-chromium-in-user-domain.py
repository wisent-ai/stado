#!/usr/bin/env python3
"""Try the browser in the user's launchd domain rather than an SSH child.

An SSH command runs in the background bootstrap: no notification centre, no
Keychain, and this patched Chromium dies there. The worker's own agents live in
`user/$uid`, which is reachable over SSH and is where browsers used to work on
this host. Submitting the same render there answers whether the domain is the
difference.

Read-only: renders `about:blank` to a file and reports the size.
"""

import os
import pathlib
import subprocess
import sys
import time

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOT = HOME / ".local" / "share" / "weles-chromium"
LABEL = "com.wisent.chromium-domain-probe"
SHOT = HOME / ".local" / "state" / "weles" / "chromium-domain-probe.png"
PROFILE = HOME / ".local" / "state" / "weles" / "chromium-domain-probe-profile"
DOMAIN = f"user/{os.getuid()}"
SETTLE = len("s" * 20)


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return (proc.stdout + proc.stderr).strip()


def main():
    builds = sorted(ROOT.glob("*/Chromium.app/Contents/MacOS/Chromium"))
    if not builds:
        raise SystemExit(f"no chromium under {ROOT}")
    binary = builds[-len(["newest"])]
    SHOT.parent.mkdir(parents=True, exist_ok=True)
    PROFILE.mkdir(parents=True, exist_ok=True)
    if SHOT.is_file():
        SHOT.unlink()
    run("/bin/launchctl", "bootout", f"{DOMAIN}/{LABEL}")
    submitted = run(
        "/bin/launchctl",
        "submit",
        "-l",
        LABEL,
        "--",
        str(binary),
        "--headless=new",
        f"--user-data-dir={PROFILE}",
        "--no-first-run",
        f"--screenshot={SHOT}",
        "about:blank",
    )
    print(f"submit      {submitted or 'ok'}")
    for _ in range(SETTLE):
        time.sleep(len("s"))
        if SHOT.is_file() and SHOT.stat().st_size:
            break
    size = SHOT.stat().st_size if SHOT.is_file() else ZERO
    print(f"screenshot  {size} bytes")
    printed = run("/bin/launchctl", "print", f"{DOMAIN}/{LABEL}")
    for key in ("state", "last exit code"):
        for line in printed.splitlines():
            if line.strip().startswith(key):
                print(f"  {line.strip()[: len('a' * 80)]}")
                break
    run("/bin/launchctl", "remove", LABEL)
    print("verdict     " + ("renders in the user domain" if size else "no render in the user domain"))
    return NONE


sys.exit(main())
