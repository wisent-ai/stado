#!/usr/bin/env python3
import gzip
import pathlib
import stat
import sys
import tarfile

source = pathlib.Path(sys.argv[1]).resolve()
destination = pathlib.Path(sys.argv[2]).resolve()
destination.parent.mkdir(parents=True, exist_ok=True)

with destination.open("wb") as raw:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for path in [source, *sorted(source.rglob("*"), key=lambda item: item.relative_to(source).as_posix())]:
                relative = pathlib.Path(source.name) / path.relative_to(source)
                info = archive.gettarinfo(str(path), arcname=relative.as_posix())
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                info.mtime = 0
                if info.isfile():
                    info.mode = 0o755 if path.stat().st_mode & stat.S_IXUSR else 0o644
                    with path.open("rb") as handle:
                        archive.addfile(info, handle)
                else:
                    archive.addfile(info)
