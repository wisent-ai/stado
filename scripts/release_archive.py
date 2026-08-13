#!/usr/bin/env python3
"""Create one deterministic immutable product archive from fixed release bytes."""

from __future__ import annotations

import argparse
import gzip
import io
import pathlib
import tarfile


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("members", nargs="+")
    args = parser.parse_args()

    root = args.root.resolve(strict=True)
    if len(set(args.members)) != len(args.members):
        raise SystemExit("archive members must be unique")

    entries: list[tuple[str, bytes, int]] = []
    for name in args.members:
        if pathlib.PurePosixPath(name).parts != (name,):
            raise SystemExit(f"archive member must be one safe root name: {name}")
        source = root / name
        if source.is_symlink() or not source.is_file():
            raise SystemExit(f"archive member is not a regular file: {name}")
        data = source.read_bytes()
        if not data:
            raise SystemExit(f"archive member is empty: {name}")
        mode = 0o755 if name != "LICENSE" else 0o644
        entries.append((name, data, mode))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
                for name, data, mode in entries:
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    info.mode = mode
                    info.uid = 0
                    info.gid = 0
                    info.uname = ""
                    info.gname = ""
                    info.mtime = 0
                    archive.addfile(info, io.BytesIO(data))


if __name__ == "__main__":
    main()
