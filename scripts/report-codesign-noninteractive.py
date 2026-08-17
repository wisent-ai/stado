#!/usr/bin/env python3
"""Answer whether this host can codesign without asking a human anything.

Rebuilding the operator app means signing it, and signing reaches into the login
keychain. If the key's access control still needs a click, an automated rebuild
would put a modal on the operator's screen, which is exactly the interruption
the rebuild is supposed to remove. So probe with a throwaway binary first: a
signature that completes on its own proves the real one will.

Signs a copy of /bin/echo in a scratch directory, reports the verdict, and
deletes it. Touches nothing that is installed.
"""

import os
import pathlib
import shutil
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
SCRATCH = HOME / ".cache" / "stado-codesign-probe"
TIMEOUT = 30


def main():
    os.umask(0o022)
    SCRATCH.mkdir(parents=True, exist_ok=True)
    os.chmod(SCRATCH, 0o755)
    identities = subprocess.run(
        ["/usr/bin/security", "find-identity", "-v", "-p", "codesigning"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    named = [line.split('"')[1] for line in identities.splitlines() if '"' in line]
    print(f"identities {named or '(none)'}")
    if not named:
        raise SystemExit("no codesigning identity on this host")
    lock = subprocess.run(
        ["/usr/bin/security", "show-keychain-info", str(HOME / "Library/Keychains/login.keychain-db")],
        capture_output=True,
        text=True,
        check=False,
    )
    print(f"keychain   {(lock.stdout or lock.stderr).strip()[: len('a' * 90)]}")
    probe = SCRATCH / "probe-binary"
    # `copy2` also copies BSD flags, which the system binaries carry and a
    # non-root process may not set: copy the bytes and the mode only.
    shutil.copyfile("/bin/echo", probe)
    os.chmod(probe, 0o755)
    signed = subprocess.run(
        ["/usr/bin/codesign", "--force", "--sign", named[ZERO], str(probe)],
        capture_output=True,
        text=True,
        check=False,
        timeout=TIMEOUT,
    )
    detail = (signed.stderr or signed.stdout).strip().splitlines()[-1:] or ["(silent)"]
    print(f"codesign   exit {signed.returncode}: {detail[ZERO][: len('a' * 90)]}")
    probe.unlink(missing_ok=True)
    print(
        "verdict    "
        + (
            "signing completes without interaction"
            if signed.returncode == ZERO
            else "signing did not complete on its own; do not rebuild unattended"
        )
    )
    return NONE


sys.exit(main())
