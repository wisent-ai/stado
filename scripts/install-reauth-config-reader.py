#!/usr/bin/env python3
"""Put the delivered reauth files into the Weles tree this host runs.

The reauth runners live in `~/weles`, not in a checkout an operator can edit, so
a repair reaches them as a delivered file plus this copy step. Existing files are
kept beside the new ones, because a runner that regresses must be restorable by
copying a file rather than by rebuilding it.

Copies only what was delivered, and prints the size and destination of each.
"""

import datetime
import hashlib
import os
import pathlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
DELIVERED = HOME / ".stado" / "files"
TREE = HOME / "weles"
FILES = {
    "reauth_config.mjs": TREE / "scripts" / "trajectories" / "_shared" / "reauth_config.mjs",
    "codex-reauth.mjs": TREE / "scripts" / "trajectories" / "codex" / "reauth.mjs",
    "claude-reauth.mjs": TREE / "scripts" / "trajectories" / "claude" / "reauth.mjs",
    "kimi-reauth.mjs": TREE / "scripts" / "trajectories" / "kimi" / "reauth.mjs",
    # The launcher is part of the repair: it sourced an env file that no longer
    # exists, so the scheduled job died before reaching the runner.
    "codex-reauth-launch.sh": TREE
    / "scripts"
    / "worker"
    / "deploy"
    / "codex-reauth"
    / "reauth-launch.sh",
    "kimi-reauth-launch.sh": TREE
    / "scripts"
    / "worker"
    / "deploy"
    / "kimi-reauth"
    / "reauth-launch.sh",
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()[: len("a" * 12)]


def main():
    if not TREE.is_dir():
        raise SystemExit(f"no weles tree at {TREE}")
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    for name, destination in FILES.items():
        source = DELIVERED / name
        if not source.is_file():
            print(f"{name:<20} not delivered; leaving {destination.name} alone")
            continue
        if destination.is_file() and digest(destination) == digest(source):
            print(f"{name:<20} settled at {destination}")
            continue
        destination.parent.mkdir(parents=True, exist_ok=True)
        if destination.is_file():
            keep = destination.with_name(f"{destination.name}.before-{stamp}")
            keep.write_bytes(destination.read_bytes())
            print(f"{name:<20} kept {keep.name}")
        destination.write_bytes(source.read_bytes())
        # A launcher that is not executable is a job that fails at exec.
        os.chmod(destination, 0o755 if destination.suffix == ".sh" else 0o644)
        print(f"{name:<20} -> {destination} ({destination.stat().st_size} bytes, {digest(destination)})")
    return NONE


sys.exit(main())
