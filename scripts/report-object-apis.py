#!/usr/bin/env python3
"""Name every object-API listener on this host and the registry each serves.

Two servers answering on one port number at two addresses is not a duplicate
anybody notices: each caller reaches whichever address its adapter names, and
both replies look canonical. The registry digest each one returns is what makes
the split visible, so print the listener, the program behind it, and the digest
of the registry object it serves.

Read-only: it inspects processes and issues GETs.
"""

import hashlib
import json
import subprocess
import sys
import urllib.error
import urllib.request

NONE = None
PORT = "8765"
BLOB_PATHS = ("/v1/objects/registry.json", "/v1/objects/ecosystem/probierz/registry.json")
TIMEOUT = len("aaaaa")


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def listeners():
    found = []
    for line in run("/usr/sbin/lsof", "-nP", f"-iTCP:{PORT}", "-sTCP:LISTEN").splitlines()[1:]:
        fields = line.split()
        if len(fields) > len(["cmd", "pid", "user", "fd", "type", "dev", "size", "node"]):
            found.append((fields[0], fields[1], fields[-2]))
    return found


def command_of(pid):
    text = run("/bin/ps", "-p", pid, "-o", "command=")
    return text.strip().splitlines()[0] if text.strip() else "(gone)"


def digest_at(address):
    for path in BLOB_PATHS:
        url = f"http://{address}{path}"
        try:
            with urllib.request.urlopen(url, timeout=TIMEOUT) as answer:
                body = answer.read()
        except (urllib.error.URLError, OSError) as error:
            yield path, f"unreachable: {error}"
            continue
        try:
            document = json.loads(body)
            targets = ",".join(
                str(entry.get("release_platform")) for entry in document.get("targets", [])
            )
        except ValueError:
            targets = "(not a registry)"
        yield path, f"sha256 {hashlib.sha256(body).hexdigest()[:12]}  platforms {targets}"


def main():
    for command, pid, address in listeners():
        print(f"{address:<26} {command} pid {pid}")
        print(f"{'':<26} {command_of(pid)[:120]}")
        for path, verdict in digest_at(address):
            print(f"{'':<26} {path} -> {verdict}")
    return NONE


sys.exit(main())
