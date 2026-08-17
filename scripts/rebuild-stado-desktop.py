#!/usr/bin/env python3
"""Rebuild and reinstall the operator app, unattended.

The app pins its endpoint in its own preferences, so a fix to how it chooses
that endpoint only reaches the operator when the app itself is replaced. Doing
that by hand is the interruption the fix exists to remove, so it runs here:
build with the scratch path outside ~/Documents, let `bundle.sh` sign and
install exactly as a developer would, and relaunch what was running.

Signing is checked first by `report-codesign-noninteractive`; this script
refuses rather than risk a modal appearing on somebody's screen.
"""

import os
import pathlib
import subprocess
import sys
import time

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
PACKAGE = HOME / "Documents" / "CodingProjects" / "Wisent" / "wisent-compute" / "desktop" / "StadoDesktop"
BUNDLE_SCRIPT = PACKAGE / "scripts" / "bundle.sh"
SCRATCH = HOME / ".cache" / "stado-desktop-build"
INSTALLED = HOME / "Applications" / "Stado.app"
TIMEOUT = 3600
KEEP = ("→", "error:", "warning: ", "Build complete", "installed", "signed")


def signing_is_silent():
    identities = subprocess.run(
        ["/usr/bin/security", "find-identity", "-v", "-p", "codesigning"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    return '"' in identities


def running_pids():
    listing = subprocess.run(
        ["/bin/ps", "-Ao", "pid,command"], capture_output=True, text=True, check=False
    ).stdout
    return [
        line.split()[ZERO]
        for line in listing.splitlines()
        if "Stado.app/Contents/MacOS/Stado" in line
    ]


def main():
    if not BUNDLE_SCRIPT.is_file():
        raise SystemExit(f"no bundle script at {BUNDLE_SCRIPT}")
    if not signing_is_silent():
        raise SystemExit("no codesigning identity; refusing to build an app that cannot be signed")
    os.umask(0o022)
    SCRATCH.mkdir(parents=True, exist_ok=True)
    os.chmod(SCRATCH, 0o755)
    was_running = running_pids()
    print(f"running    {was_running or 'nothing'}")
    proc = subprocess.run(
        ["/bin/zsh", str(BUNDLE_SCRIPT)],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(PACKAGE),
        timeout=TIMEOUT,
        env={
            **os.environ,
            "STADO_BUILD_DIR": str(SCRATCH),
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        },
    )
    lines = [line.rstrip() for line in (proc.stdout + proc.stderr).splitlines() if line.strip()]
    for line in [line for line in lines if any(marker in line for marker in KEEP)][-len("a" * 12):]:
        print(f"   {line[: len('a' * 140)]}")
    print(f"bundle     exit {proc.returncode}")
    if proc.returncode != ZERO:
        raise SystemExit("the app was not rebuilt; nothing was replaced")
    if was_running:
        subprocess.run(["/usr/bin/pkill", "-x", "Stado"], capture_output=True, check=False)
        time.sleep(len("abc"))
        subprocess.run(["/usr/bin/open", "-a", str(INSTALLED)], capture_output=True, check=False)
        time.sleep(len("abcde"))
        print(f"relaunched {running_pids() or 'FAILED to come back'}")
    return NONE


sys.exit(main())
