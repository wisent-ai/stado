#!/usr/bin/env python3
"""Point Weles capture callers at the synchronous Weles API.

Idempotently rewrites the canonical service-directory entry retained under the
historic ``weles-admission`` key. The database admission server was removed;
port 8788 is the managed Weles API that executes ``generic_capture`` via /run.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

SERVICE = "weles-admission"
HOST = "charless-mac-mini"
URL = "http://127.0.0.1:8788"
MANAGED_SERVICE = "com.wisent.always-on.weles-api"


def run(*args: str, input_text: str | None = None) -> str:
    result = subprocess.run(args, input=input_text, text=True, capture_output=True)
    if result.returncode != 0:
        raise SystemExit(result.stderr.strip() or result.stdout.strip())
    return result.stdout


def main() -> None:
    document = json.loads(run("stado", "registry", "pull"))
    directory = document["service_directory"]
    service = directory["services"][SERVICE]
    endpoints = service.setdefault("endpoints", {})
    changed = endpoints.get(HOST, {}).get("url") != URL
    changed = changed or service.get("managed_service") != MANAGED_SERVICE
    if not changed:
        print(f"{SERVICE}: already {URL}")
        return

    endpoints[HOST] = {"url": URL}
    service["managed_service"] = MANAGED_SERVICE
    directory["generation"] = int(directory["generation"]) + 1

    payload = Path(__file__).resolve().parent / ".weles-capture-registry.json"
    try:
        payload.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
        print(run("stado", "registry", "push", str(payload)).strip())
    finally:
        payload.unlink(missing_ok=True)

    print(f"{SERVICE}: {URL}; generation {directory['generation']}")


if __name__ == "__main__":
    main()
