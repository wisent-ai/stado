#!/usr/bin/env python3
"""Say what each reachable operations endpoint reports about workers.

The desktop app renders `/api/state.json`, and three different processes can
answer that path on this fleet: an operator laptop's host-health API, the
authority host's object API, and a resolver adapter in front of the latter. They
are not interchangeable, and today they disagree -- one says three workers, two
of them unavailable, another says none at all -- so the operator's screen is
decided by which one the app happens to hold.

Read-only. Prints, per endpoint: the worker rows with their status and the age
of the capacity report each row is derived from.
"""

import datetime
import json
import os
import pathlib
import sys
import urllib.error
import urllib.request

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
TOKEN_FILE = HOME / ".stado" / "wisent-queue-object-api-token"
ENDPOINTS = [
    address
    for address in os.environ.get(
        "OPERATIONS_ENDPOINTS", "http://127.0.0.1:8765,http://127.0.0.1:18765,http://127.0.0.1:18776"
    ).split(",")
    if address.strip()
]
TIMEOUT = 20


def ask(base, path):
    token = TOKEN_FILE.read_text(encoding="utf-8").strip() if TOKEN_FILE.is_file() else ""
    request = urllib.request.Request(
        f"{base.rstrip('/')}/{path}", headers={"Authorization": f"Bearer {token}"}
    )
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as answer:
            return answer.status, answer.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read() or b""
    except OSError as error:
        return f"unreachable ({error})", b""


def main():
    now = datetime.datetime.now(datetime.timezone.utc)
    for base in ENDPOINTS:
        status, body = ask(base, "api/state.json")
        print(f"== {base}  http {status}  {len(body)} bytes")
        if not isinstance(status, int) or status != 200:
            continue
        try:
            document = json.loads(body)
        except ValueError as problem:
            print(f"   unparseable: {problem}")
            continue
        workers = document.get("workers") or []
        print(f"   workers {len(workers)}  queue {document.get('queue') or document.get('jobs')}")
        for worker in workers[: len("a" * 6)]:
            stamp = worker.get("last_report") or worker.get("published_at") or worker.get("observed_at")
            age = "unstamped"
            if isinstance(stamp, str):
                try:
                    age = f"{int((now - datetime.datetime.fromisoformat(stamp.replace('Z', '+00:00'))).total_seconds())}s"
                except ValueError:
                    age = stamp[: len("a" * 24)]
            print(
                f"     {str(worker.get('name') or worker.get('target') or worker.get('consumer_id'))[: len('a' * 30)]:32}"
                f" {str(worker.get('status')):12} report {age}"
            )
    return NONE


sys.exit(main())
