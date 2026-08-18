#!/usr/bin/env python3
"""Authorize one resolver key on this host, idempotently.

The resolver on an operator machine reads the registry from this host over SSH.
It must do that with its own key, not with whatever agent a human happened to
have loaded, or the service works when started by hand and fails when started by
launchd -- which is exactly how the fleet's client side went dark today.

Reads the public key from an owner-only file delivered by
`stado host install-secret` (a public key is not secret, but the delivery path
is the one that exists), appends it to `~/.ssh/authorized_keys` if absent, and
prints only fingerprints.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
# `stado host install-file` lands its payload under ~/.stado/files, and helpers
# do not inherit the caller's environment, so both places are checked rather
# than requiring a variable that cannot arrive.
CANDIDATES = (
    pathlib.Path(os.environ.get("RESOLVER_PUBKEY_FILE", "")),
    HOME / ".stado" / "files" / "resolver-ssh-key.pub",
    HOME / ".stado" / "resolver-ssh-key.pub",
)
DELIVERED = next(
    (path for path in CANDIDATES if str(path) and path.is_file()), CANDIDATES[len("a")]
)
AUTHORIZED = HOME / ".ssh" / "authorized_keys"


def fingerprint(line):
    proc = subprocess.run(
        ["/usr/bin/ssh-keygen", "-lf", "-"], input=line, capture_output=True, text=True, check=False
    )
    return (proc.stdout or proc.stderr).strip()[: len("a" * 90)]


def main():
    if not DELIVERED.is_file():
        raise SystemExit(f"no delivered public key at {DELIVERED}")
    offered = DELIVERED.read_text(encoding="utf-8").strip()
    if not offered.startswith("ssh-"):
        raise SystemExit("the delivered file does not look like an OpenSSH public key")
    print(f"offered    {fingerprint(offered)}")
    AUTHORIZED.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    existing = AUTHORIZED.read_text(encoding="utf-8") if AUTHORIZED.is_file() else ""
    material = offered.split()[len("a")] if len(offered.split()) > len("a") else offered
    if material in existing:
        print("settled    this key is already authorized here")
        return NONE
    with AUTHORIZED.open("a", encoding="utf-8") as handle:
        if existing and not existing.endswith("\n"):
            handle.write("\n")
        handle.write(offered + "\n")
    os.chmod(AUTHORIZED, 0o600)
    print(f"authorized {AUTHORIZED} now carries {len(existing.splitlines()) + 1} keys")
    return NONE


sys.exit(main())
