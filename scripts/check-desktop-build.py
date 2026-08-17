#!/usr/bin/env python3
"""Type-check the operator app the same way the control plane is checked.

Same two constraints as `check-stado-build.py`: this session cannot write inside
~/Documents, and the host agent runs helpers under a umask that strips the
execute bit from directories a build creates. So the checkout stays where it is,
the build products go to a scratch path outside Documents, and the umask is
relaxed for this process only.
"""

import os
import pathlib
import shutil
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
PACKAGE = HOME / "Documents" / "CodingProjects" / "Wisent" / "wisent-compute" / "desktop" / "StadoDesktop"
SCRATCH = HOME / ".cache" / "stado-desktop-build"
SWIFT = "/usr/bin/swift"
TIMEOUT = 3600
KEEP = ("error:", "warning:", "Compiling", "Build complete", "Planning build")


def seed_from_checkout():
    """Copy the package's already-resolved dependencies into the scratch path.

    SwiftPM in a helper has no credentials for the private repositories this app
    depends on, so a fresh scratch cannot check out `wisent-errors` and friends
    and the build dies before it reads a line of Swift. The developer checkout
    beside the package already holds those revisions, and ~/Documents is
    readable here even though it is not writable, so the resolved state is
    copied once and the build then runs offline.
    """
    source = PACKAGE / ".build"
    if not source.is_dir():
        return "no resolved checkout to seed from"
    copied = []
    for name in ("checkouts", "repositories", "artifacts"):
        origin = source / name
        target = SCRATCH / name
        if not origin.is_dir():
            continue
        # Replace rather than skip: a previous failed run leaves these
        # directories present but without the revisions the manifest pins, and
        # "already there" then means "still cannot build".
        if target.is_dir():
            shutil.rmtree(target)
        shutil.copytree(origin, target, symlinks=True, ignore_dangling_symlinks=True)
        copied.append(name)
    state = source / "workspace-state.json"
    if state.is_file():
        shutil.copy2(state, SCRATCH / state.name)
        copied.append(state.name)
    return f"seeded {', '.join(copied)}" if copied else "scratch already carries the dependencies"


def main():
    if not (PACKAGE / "Package.swift").is_file():
        raise SystemExit(f"no SwiftPM package at {PACKAGE}")
    os.umask(0o022)
    SCRATCH.mkdir(parents=True, exist_ok=True)
    os.chmod(SCRATCH, 0o755)
    print(seed_from_checkout())
    proc = subprocess.run(
        [
            SWIFT,
            "build",
            "--package-path",
            str(PACKAGE),
            "--scratch-path",
            str(SCRATCH),
            "--skip-update",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=TIMEOUT,
        env={**os.environ, "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"},
    )
    lines = [line.rstrip() for line in (proc.stdout + proc.stderr).splitlines() if line.strip()]
    picked = [line for line in lines if any(marker in line for marker in KEEP)]
    for line in (picked or lines)[-len("a" * 30):]:
        print(line[: len("a" * 165)])
    print(f"exit {proc.returncode}")
    return NONE


sys.exit(main())
