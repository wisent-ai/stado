#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import os
import pathlib
import subprocess
import tarfile
import tempfile


def required(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise RuntimeError(f"Stado delivery did not provide {name}")
    return value


def safe_extract_member(bundle: tarfile.TarFile, basename: str, destination: pathlib.Path) -> None:
    entry = next(
        (
            item
            for item in bundle.getmembers()
            if item.isfile()
            and (
                pathlib.PurePosixPath(item.name).as_posix() == basename
                or pathlib.PurePosixPath(item.name).as_posix().endswith("/" + basename)
            )
        ),
        None,
    )
    if entry is None or entry.issym() or entry.islnk():
        raise RuntimeError(f"canonical Stado archive has no regular {basename}")
    payload = bundle.extractfile(entry)
    if payload is None:
        raise RuntimeError(f"canonical Stado archive member {basename} is unreadable")
    destination.write_bytes(payload.read())
    destination.chmod(0o755)

def stado_binary() -> str:
    """The stado CLI as the delivery host actually carries it.

    A delivery job runs under an agent whose PATH is the supervisor's minimal
    one, so the bare word `stado` raised FileNotFoundError and the fleet's
    own release could not deliver itself. The owner install is the canonical
    location; PATH remains the fallback for operator shells.
    """
    owner = pathlib.Path.home() / ".stado" / "bin" / "stado"
    if owner.is_file() and os.access(owner, os.X_OK):
        return str(owner)
    return "stado"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", action="append", required=True)
    args = parser.parse_args()
    archive = pathlib.Path(required("WISENT_RELEASE_ARCHIVE"))
    expected = required("WISENT_RELEASE_SHA256")
    if hashlib.sha256(archive.read_bytes()).hexdigest() != expected:
        raise RuntimeError("canonical Stado archive digest mismatch")
    with tempfile.TemporaryDirectory(prefix="stado-fleet-release-") as temporary:
        binary = pathlib.Path(temporary) / "stado"
        with tarfile.open(archive, "r:gz") as bundle:
            safe_extract_member(bundle, "bin/stado", binary)
        for target in args.target:
            subprocess.run(
                [stado_binary(), "host", "install-binary", target, "--from", str(binary), "--name", "stado", "--json"],
                check=True,
            )


if __name__ == "__main__":
    main()
