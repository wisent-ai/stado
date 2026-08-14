#!/usr/bin/env python3
"""Build the node-pty native module this host's login helpers need.

Every provider login drives its CLI on a pseudo-terminal, and node-pty needs a
compiled binding plus its `spawn-helper` to open one. The deployed release ships
the package's JavaScript and no `build/` directory at all, so `import` succeeds,
the failure waits until the first spawn, and it reads as `posix_spawnp failed` --
a message that names neither the module nor the missing file.

Builds in place with npm, then reports whether the two artifacts exist. Safe to
re-run: npm rebuilds only what is missing or stale.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
TREE = HOME / "weles"
MODULE = TREE / "node_modules" / "node-pty"
NPM = "/opt/homebrew/bin/npm"
ARTIFACTS = ("build/Release/pty.node", "build/Release/spawn-helper")
MINUTES = float(len("m" * 600))


def state():
    return {name: (MODULE / name).is_file() for name in ARTIFACTS}


def main():
    if not MODULE.is_dir():
        raise SystemExit(f"no node-pty at {MODULE}")
    before = state()
    print(f"before     {before}")
    if all(before.values()):
        print("settled    the binding and its spawn-helper are already built")
        return NONE
    proc = subprocess.run(
        [NPM, "rebuild", "node-pty", "--build-from-source"],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(TREE),
        timeout=MINUTES,
        env={
            **os.environ,
            "HOME": str(HOME),
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
            "npm_config_build_from_source": "true",
        },
    )
    tail = [line for line in (proc.stdout + proc.stderr).splitlines() if line.strip()][-len("aaaaaaaa"):]
    print(f"npm exit   {proc.returncode}")
    for line in tail:
        print(f"  {line[: len('a' * 180)]}")
    after = state()
    print(f"after      {after}")
    print("verdict    built" if all(after.values()) else "verdict    still missing")
    return NONE


sys.exit(main())
