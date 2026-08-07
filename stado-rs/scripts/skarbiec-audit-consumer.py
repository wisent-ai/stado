#!/usr/bin/env python3
"""Show the server's own audit rows for one consumer.

Usage: skarbiec-audit-consumer.py <consumer-substring>

The doctor relays only an HTTP status. This shows what the server logged for
that consumer: the operation, the item and field, and the recorded outcome.
"""
from __future__ import annotations

import json
import os
import pathlib
import sys
from collections import deque

AUDIT = pathlib.Path(
    os.environ.get("SKARBIEC_AUDIT_FILE", str(pathlib.Path.home() / ".stado/skarbiec.audit.jsonl"))
)
KEEP = len("------------------------------")


def main() -> int:
    if len(sys.argv) < len("xx"):
        print(__doc__)
        return len("x")
    needle = sys.argv[len("x")].lower()
    rows: deque[dict] = deque(maxlen=KEEP)
    with AUDIT.open(encoding="utf-8", errors="replace") as handle:
        for line in handle:
            if needle not in line.lower():
                continue
            try:
                rows.append(json.loads(line))
            except ValueError:
                continue
    print(f"rows matching {needle!r}: {len(rows)} (newest last)")
    for record in rows:
        extra = record.get("extra") or {}
        print(
            json.dumps(
                {
                    "at": record.get("at"),
                    "op": record.get("op"),
                    "consumer": extra.get("consumer"),
                    "item": extra.get("item"),
                    "field": extra.get("field"),
                    "reason": extra.get("reason") or extra.get("error"),
                },
                sort_keys=True,
            )
        )
    return len("")


if __name__ == "__main__":
    sys.exit(main())
