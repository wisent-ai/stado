#!/usr/bin/env python3
"""Compare what the authority PUBLISHES with what it READS.

A resolver refuses a directory that changed without advancing its generation,
and it fetches that directory from the authority's `resolver snapshot`. If that
command answers from a different document than the authority's own
`registry pull`, the refusal is permanent and no edit can clear it: one side
keeps changing while the number that describes it does not.

Prints the generation, digest and service-endpoint list from both paths.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
TIMEOUT = 120


def described(document):
    directory = document.get("service_directory") or {}
    services = directory.get("services") or {}
    endpoints = {
        name: sorted((entry.get("endpoints") or {}).keys()) for name, entry in services.items()
    }
    return {
        "generation": directory.get("generation"),
        "digest": hashlib.sha256(json.dumps(directory, sort_keys=True).encode()).hexdigest()[
            : len("a" * 16)
        ],
        "object_api_hosts": endpoints.get("stado-object-api"),
        "targets": len(document.get("targets") or []),
    }


def read(argv):
    proc = subprocess.run(
        [str(STADO), *argv], capture_output=True, text=True, check=False, timeout=TIMEOUT
    )
    if proc.returncode != ZERO:
        return {"error": (proc.stderr or proc.stdout).strip().splitlines()[-1:]}
    try:
        payload = json.loads(proc.stdout)
    except ValueError as problem:
        return {"error": f"not JSON: {problem}"}
    # `resolver snapshot` wraps the document; `registry pull` prints it bare.
    document = payload.get("document") if isinstance(payload, dict) and "document" in payload else payload
    return described(document)


def main():
    print(f"registry pull      {json.dumps(read(['registry', 'pull']))}")
    print(f"resolver snapshot  {json.dumps(read(['resolver', 'snapshot']))}")
    return NONE


sys.exit(main())
