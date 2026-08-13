#!/usr/bin/env python3
"""Say what the tailnet object proxy forwards to, and what it answers.

Off-host callers reach the fleet's object API through a TLS proxy on the tailnet
address while the API itself listens on loopback. When the proxy answers 503 and
the API answers 200 for the same object, the two are not looking at the same
place -- and every remote host then falls back to reading its own disk. This
prints the proxy's target as its own source states it, and the status each side
returns for the registry object.

Read-only: it reads the proxy script and issues GETs.
"""

import json
import os
import pathlib
import re
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
PROXY = HOME / ".stado" / "bin" / "stado-tailnet-object-proxy"
OBJECT = "/api/object?uri=stado%3A%2F%2Fprobierz%2Fregistry.json"
TOKEN = HOME / ".stado" / "wisent-queue-object-api-token"


def targets_in(text):
    """Every URL or host:port the proxy names, in source order."""
    found = re.findall(r"https?://[^\s\"']+|127\.0\.0\.1:\d+|localhost:\d+", text)
    seen = []
    for item in found:
        if item not in seen:
            seen.append(item)
    return seen


def status(url, insecure=False):
    args = ["/usr/bin/curl", "-s", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "10"]
    if insecure:
        args.append("-k")
    if TOKEN.is_file():
        args += ["-H", f"Authorization: Bearer {TOKEN.read_text(encoding='utf-8').strip()}"]
    proc = subprocess.run(args + [url], capture_output=True, text=True, check=False)
    return proc.stdout.strip() or proc.stderr.strip()[: len("aaaaaaaaaaaaaaaaaaaa")]


def main():
    if PROXY.is_file():
        text = PROXY.read_text(encoding="utf-8", errors="replace")
        print(f"proxy       {PROXY}")
        print(f"forwards to {' '.join(targets_in(text)) or '(nothing recognisable)'}")
        env = re.findall(r"process\.env\.([A-Z0-9_]+)", text)
        print(f"reads env   {' '.join(sorted(set(env))) or '(none)'}")
    else:
        print(f"proxy       {PROXY} (absent)")
    print(f"loopback    {status('http://127.0.0.1:8765' + OBJECT)}")
    print(f"tailnet     {status('https://100.120.25.24:8765' + OBJECT, insecure=True)}")
    return NONE


sys.exit(main())
