#!/usr/bin/env python3
"""Report the process owning Stado resolver's loopback TCP listener."""

from pathlib import Path
import os

PORT = 17600
LISTEN = "0A"
inodes: set[str] = set()
for table in (Path("/proc/net/tcp"), Path("/proc/net/tcp6")):
    if not table.exists():
        continue
    for line in table.read_text().splitlines()[1:]:
        fields = line.split()
        if len(fields) < 10:
            continue
        local_address, state, inode = fields[1], fields[3], fields[9]
        if int(local_address.rsplit(":", 1)[1], 16) == PORT and state == LISTEN:
            inodes.add(inode)

if not inodes:
    raise SystemExit(f"no process listens on TCP port {PORT}")

found = False
for process_dir in Path("/proc").iterdir():
    if not process_dir.name.isdigit():
        continue
    try:
        descriptors = process_dir.joinpath("fd").iterdir()
        owns_listener = any(
            os.readlink(descriptor) in {f"socket:[{inode}]" for inode in inodes}
            for descriptor in descriptors
        )
    except (FileNotFoundError, PermissionError, ProcessLookupError):
        continue
    if not owns_listener:
        continue
    found = True
    command = process_dir.joinpath("comm").read_text().strip()
    cmdline = process_dir.joinpath("cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace").strip()
    print(f"pid={process_dir.name} command={command} cmdline={cmdline}")

if not found:
    raise SystemExit(f"TCP port {PORT} is listening, but its owner was not visible")
