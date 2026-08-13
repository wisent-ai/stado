#!/usr/bin/env python3
"""Say which version of each reauth runner this host is actually running.

"Delivered" and "in place" have diverged twice during this repair, and a job that
fails with the old message while the new file sits in the delivery directory is
indistinguishable from a fix that did not work. Print the digest of each file the
jobs execute and whether it contains the markers the new code introduced.

Read-only.
"""

import hashlib
import os
import pathlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
TREE = HOME / "weles"
DELIVERED = HOME / ".stado" / "files"
TARGETS = {
    "codex runner": (
        TREE / "scripts" / "trajectories" / "codex" / "reauth.mjs",
        DELIVERED / "codex-reauth.mjs",
    ),
    "claude runner": (
        TREE / "scripts" / "trajectories" / "claude" / "reauth.mjs",
        DELIVERED / "claude-reauth.mjs",
    ),
    "shared config": (
        TREE / "scripts" / "trajectories" / "_shared" / "reauth_config.mjs",
        DELIVERED / "reauth_config.mjs",
    ),
    "codex launcher": (
        TREE / "scripts" / "worker" / "deploy" / "codex-reauth" / "reauth-launch.sh",
        DELIVERED / "codex-reauth-launch.sh",
    ),
}
MARKERS = ("configFromSkarbiec", "loadFromSkarbiec", "resolveBearer", "WELES_WORKER_ENV_FILE")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()[: len("a" * 12)] if path.is_file() else "absent"


def main():
    for label, (live, delivered) in TARGETS.items():
        text = live.read_text(encoding="utf-8", errors="replace") if live.is_file() else ""
        present = [marker for marker in MARKERS if marker in text]
        same = digest(live) == digest(delivered)
        print(f"{label:<16} live {digest(live)}  delivered {digest(delivered)}  {'same' if same else 'DIFFERENT'}")
        print(f"{'':<16} markers {present or '(none)'}")
    return NONE


sys.exit(main())
