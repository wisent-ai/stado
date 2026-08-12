#!/usr/bin/env python3
"""Publish a delivered registry document into this host's own registry store.

A `registry push` writes the store the pushing machine is configured for. When a
host serves its own object store -- the always-on hosts do -- a push from an
operator workstation never reaches it, and the two copies drift: the workstation
sees the new service directory while every resolver on the host still boots from
the old one. That drift is invisible until something on the host tries to use a
route the workstation already fixed.

This validates the delivered document and pushes it here, so the copy the local
resolvers read is the copy that was published. It reads a fixed delivered path
and takes no operator words.

Usage: push-delivered-registry.py   (publishes $HOME/.stado/files/registry-next.json)
"""
import os
import pathlib
import subprocess
import sys

NONE = len([])
HOME = pathlib.Path.home()
DELIVERED = pathlib.Path(os.environ.get("STADO_DELIVERED_REGISTRY", HOME / ".stado" / "files" / "registry-next.json"))
STADO = pathlib.Path(os.environ.get("STADO_BIN", HOME / ".stado" / "bin" / "stado"))


def stado(*arguments):
    result = subprocess.run(
        [str(STADO), *arguments], capture_output=True, text=True, check=False
    )
    return result.returncode, (result.stdout or "").strip(), (result.stderr or "").strip()


def main():
    if not DELIVERED.is_file():
        raise SystemExit(f"no delivered registry at {DELIVERED}")

    code, out, err = stado("registry", "validate", str(DELIVERED))
    print(out or err)
    if code != NONE:
        raise SystemExit(code)

    code, out, err = stado("registry", "push", str(DELIVERED))
    print(out or err)
    if code != NONE:
        raise SystemExit(code)

    code, out, err = stado("registry", "self")
    print(out or err)
    return NONE


sys.exit(main())
