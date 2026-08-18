#!/usr/bin/env python3
"""Make this host's local registry copy a copy again.

An accidental push built from the local copy left it AHEAD of the authority:
generation 12 here, 8 there. The resolver bootstraps from the local copy and
then fetches the authority's document, sees the generation go backwards, and
refuses every connection -- so the operator's whole CLI stops while both stores
are individually healthy.

Run this ON the authority host: it reads the canonical document through the
store the authority itself uses, and writes it into the named host's object API,
so the copy carries exactly what the fleet carries. Prints generations and
digests, never contents.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys
import urllib.error
import urllib.request

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
TARGET_URL = os.environ.get("ALIGN_COPY_URL", "")
TOKEN_FILE = pathlib.Path(
    os.environ.get("ALIGN_COPY_TOKEN_FILE", HOME / ".stado" / "wisent-queue-object-api-token")
)
URI = "stado://probierz/registry.json"
TIMEOUT = 60


def digest(payload):
    return hashlib.sha256(payload).hexdigest()[: len("a" * 16)]


def canonical():
    proc = subprocess.run(
        [str(STADO), "registry", "pull"], capture_output=True, text=True, check=False, timeout=TIMEOUT
    )
    if proc.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {(proc.stderr or proc.stdout).strip()[:160]}")
    return proc.stdout.encode()


def put(url, payload):
    token = TOKEN_FILE.read_text(encoding="utf-8").strip() if TOKEN_FILE.is_file() else ""
    request = urllib.request.Request(
        f"{url.rstrip('/')}/api/object?uri={urllib.parse.quote(URI, safe='')}",
        data=payload,
        method="PUT",
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as answer:
            return answer.status, answer.read()[: len("a" * 200)]
    except urllib.error.HTTPError as error:
        return error.code, error.read()[: len("a" * 200)]
    except OSError as error:
        return f"unreachable ({error})", b""


def main():
    import urllib.parse  # noqa: F401

    if not TARGET_URL:
        raise SystemExit("ALIGN_COPY_URL is required: the object API of the host whose copy to align")
    payload = canonical()
    document = json.loads(payload)
    print(f"canonical  generation {(document.get('service_directory') or {}).get('generation')} digest {digest(payload)}")
    status, body = put(TARGET_URL, payload)
    print(f"put        {TARGET_URL} -> {status} {body.decode('utf-8', 'replace')[: len('a' * 120)]}")
    if status != 200:
        raise SystemExit("the copy was not replaced")
    return NONE


import urllib.parse

sys.exit(main())
