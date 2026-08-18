#!/usr/bin/env python3
"""Give the resolver its own SSH key instead of borrowing a login session's.

The resolver reaches the registry authority over SSH. Started by hand it
inherited `SSH_AUTH_SOCK` and worked; started by launchd it has no agent, no key
of its own, and exits with a configuration error and an empty log -- so the
fleet's whole client side went down while every manual check passed. A service
must carry its own credential.

This creates `~/.stado/resolver-ssh-key` when absent (ed25519, no passphrase,
owner-only) and prints the public half so it can be authorized on the authority.
It never touches an existing key and never prints private material.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
KEY = pathlib.Path(os.environ.get("RESOLVER_SSH_KEY", HOME / ".stado" / "resolver-ssh-key"))
COMMENT = os.environ.get("RESOLVER_SSH_COMMENT", "stado-resolver")


def main():
    if KEY.exists():
        public = KEY.with_suffix(KEY.suffix + ".pub") if KEY.suffix else pathlib.Path(f"{KEY}.pub")
        print(f"settled    {KEY} already exists")
        if public.is_file():
            print(f"public     {public.read_text(encoding='utf-8').strip()}")
        return NONE
    os.umask(0o077)
    KEY.parent.mkdir(parents=True, exist_ok=True)
    made = subprocess.run(
        [
            "/usr/bin/ssh-keygen",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            COMMENT,
            "-f",
            str(KEY),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if made.returncode != ZERO:
        raise SystemExit(f"ssh-keygen failed: {(made.stderr or made.stdout).strip()[: len('a' * 160)]}")
    os.chmod(KEY, 0o600)
    public = pathlib.Path(f"{KEY}.pub")
    os.chmod(public, 0o644)
    print(f"created    {KEY} (owner-only) and {public.name}")
    print(f"public     {public.read_text(encoding='utf-8').strip()}")
    return NONE


sys.exit(main())
