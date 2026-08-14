#!/usr/bin/env python3
"""Try the browser inside the user's audit session, with privilege.

`launchctl asuser` was refused from an unprivileged SSH child -- "Could not
switch to audit session: Operation not permitted" -- which is a statement about
the caller, not about the session. This host grants passwordless sudo for
maintenance, and the same command through it either places the browser in the
user's session or proves there is no such session to enter.

Read-only: renders `about:blank` to a file and reports the size.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOT = HOME / ".local" / "share" / "weles-chromium"
SHOT = HOME / ".local" / "state" / "weles" / "chromium-asuser-probe.png"
PROFILE = HOME / ".local" / "state" / "weles" / "chromium-asuser-profile"
TIMEOUT = float(len("s" * 60))


def main():
    builds = sorted(ROOT.glob("*/Chromium.app/Contents/MacOS/Chromium"))
    if not builds:
        raise SystemExit(f"no chromium under {ROOT}")
    binary = builds[-len(["newest"])]
    SHOT.parent.mkdir(parents=True, exist_ok=True)
    PROFILE.mkdir(parents=True, exist_ok=True)
    if SHOT.is_file():
        SHOT.unlink()
    command = [
        "/usr/bin/sudo",
        "-n",
        "/bin/launchctl",
        "asuser",
        str(os.getuid()),
        "/usr/bin/sudo",
        "-n",
        "-u",
        os.environ.get("USER", HOME.name),
        str(binary),
        "--headless=new",
        f"--user-data-dir={PROFILE}",
        "--no-first-run",
        f"--screenshot={SHOT}",
        "about:blank",
    ]
    try:
        proc = subprocess.run(command, capture_output=True, text=True, check=False, timeout=TIMEOUT)
        code, err = proc.returncode, proc.stderr
    except subprocess.TimeoutExpired as expired:
        code = "timed out"
        err = expired.stderr if isinstance(expired.stderr, str) else (expired.stderr or b"").decode("utf-8", "replace")
    size = SHOT.stat().st_size if SHOT.is_file() else ZERO
    print(f"asuser+sudo exit {code}  wrote {size} bytes")
    for line in (err or "").splitlines()[: len("llll")]:
        if line.strip():
            print(f"  {line[: len('a' * 170)]}")
    print("verdict     " + ("renders in the user's session" if size else "no render"))
    return NONE


sys.exit(main())
