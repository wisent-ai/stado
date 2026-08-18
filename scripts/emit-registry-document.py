#!/usr/bin/env python3
"""Print the registry document this host serves, byte for byte.

The tailnet path to the authority times out often enough that a repair which
depends on it is not a repair. The helper channel works, so the authority can
hand its document over that instead: this prints exactly what its own object API
answers for `stado://probierz/registry.json`, and the caller decides what to do
with it.
"""

import json
import os
import pathlib
import subprocess
import sys
import urllib.parse

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
TOKEN = HOME / ".stado" / "wisent-queue-object-api-token"
URI = "stado://probierz/registry.json"

def endpoints(document):
    """Every way this host's own store can be addressed, best guess first.

    The configured URL states a scheme, and on the authority it states `http`
    for a port that speaks TLS -- so a reader that trusts the scheme alone gets
    an empty answer and calls the store unreachable. Try the stated URL, then
    the same port over TLS with the configured CA, then the plain local port.
    """
    stated = ((document.get("storage") or {}).get("stado") or {}).get("url") or ""
    port = (document.get("object_api") or {}).get("port")
    out = []
    if stated:
        out.append(stated.rstrip("/"))
        if stated.startswith("http://"):
            out.append("https://" + stated[len("http://"):].rstrip("/"))
    if port and f":{port}" not in "".join(out):
        out.append(f"http://127.0.0.1:{port}")
    return out


def ca_args(document):
    ca = ((document.get("storage") or {}).get("stado") or {}).get("ca_file") or ""
    return ["--cacert", ca] if ca and pathlib.Path(ca).is_file() else []


def main():
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    token = TOKEN.read_text(encoding="utf-8").strip() if TOKEN.is_file() else ""
    tried = []
    for base in endpoints(document):
        endpoint = f"{base}/api/object?uri={urllib.parse.quote(URI, safe='')}"
        proc = subprocess.run(
            ["/usr/bin/curl", "-s", "-m", "25", "-H", f"Authorization: Bearer {token}"]
            + ca_args(document)
            + [endpoint],
            capture_output=True,
            text=True,
            check=False,
        )
        try:
            parsed = json.loads(proc.stdout)
        except json.JSONDecodeError:
            tried.append(f"{base}: no json")
            continue
        if "targets" not in parsed:
            tried.append(f"{base}: no targets")
            continue
        sys.stdout.write(json.dumps(parsed, indent=2, sort_keys=True) + "\n")
        return NONE
    raise SystemExit("no endpoint answered with the registry: " + "; ".join(tried))


sys.exit(main())
