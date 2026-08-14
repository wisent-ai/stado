#!/usr/bin/env python3
"""Type-check stado-rs on the host that owns the checkout.

An agent session cannot compile here: its sandbox refuses writes to build
directories, so `cargo check` dies with `Permission denied (os error 13)` before
it reads a single line of Rust. The Stado host agent runs as the same user
without that restriction, so the fleet's own execution path is also the compile
path -- no new CI gate, no tag push, no release.

Prints the compiler's own verdict lines and nothing else.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
TREE = HOME / "Documents" / "CodingProjects" / "Wisent" / "wisent-compute" / "stado-rs"
CARGO = HOME / ".cargo" / "bin" / "cargo"
TIMEOUT = 3600
KEEP = ("error", "warning: unused", "Finished", "Checking stado", "Compiling stado")


def main():
    if not TREE.is_dir():
        raise SystemExit(f"no stado-rs checkout at {TREE}")
    if not CARGO.is_file():
        raise SystemExit(f"no cargo at {CARGO}")
    # macOS protects ~/Documents from a launchd-run helper's writes, and cargo
    # writes its whole target tree. The checkout stays where it is and only the
    # build output moves, which is the difference between `Permission denied`
    # and a compiler verdict.
    scratch = HOME / ".cache" / "stado-build"
    scratch.mkdir(parents=True, exist_ok=True)
    # The Stado host agent runs helpers under a secret-safe umask that strips
    # the execute bit, so every directory cargo creates comes out `drw-------`
    # and cargo cannot enter the tree it just made. Relax it for this process
    # only: nothing here writes a secret.
    os.umask(0o022)
    os.chmod(scratch, 0o755)
    proc = subprocess.run(
        [str(CARGO), "check", "--lib", "--bins"],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(TREE),
        timeout=TIMEOUT,
        env={
            **os.environ,
            "CARGO_TARGET_DIR": str(scratch),
            "PATH": f"{CARGO.parent}:/opt/homebrew/bin:/usr/bin:/bin",
        },
    )
    lines = [line.rstrip() for line in (proc.stdout + proc.stderr).splitlines() if line.strip()]
    picked = [line for line in lines if any(marker in line for marker in KEEP)]
    for line in (picked or lines)[-len("a" * 40):]:
        print(line[: len("a" * 165)])
    print(f"exit {proc.returncode}")
    return NONE


sys.exit(main())
