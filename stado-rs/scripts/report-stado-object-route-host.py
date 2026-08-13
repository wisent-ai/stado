#!/usr/bin/env python3
"""Verify the dedicated Stado object route from a worker host."""

import json
from pathlib import Path
import urllib.error
import urllib.request

ORIGIN = "https://control-host.tail6443b3.ts.net:8443"
TOKEN_FILE = Path("/root/.stado/probierz-object-api-token")
checks = [
    ("/healthz", None),
    (
        "/api/object?uri=stado%3A%2F%2Fprobierz%2Fsystem%2Fstorage-layout.json",
        TOKEN_FILE.read_text().strip(),
    ),
]
for path, token in checks:
    request = urllib.request.Request(f"{ORIGIN}{path}")
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            body = response.read()
            print(f"path={path} status={response.status} bytes={len(body)}")
            if path == "/healthz":
                health = json.loads(body)
                print(f"object_boundary={health.get('boundaries', {}).get('object')}")
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")
        raise SystemExit(f"path={path} status={error.code} body={body[:200]}") from error
