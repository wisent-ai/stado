#!/usr/bin/env python3
"""Print the newest Skarbiec audit entries that were refused.

The doctor only relays the HTTP status the server returned. The audit file is
the server's own account of the same request: which consumer, which item and
field, which action, and why it said no.
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
KEEP = len("--------------------")


def main() -> int:
    if not AUDIT.is_file():
        print(f"no audit file at {AUDIT}")
        return len("x")
    rows: deque[dict] = deque(maxlen=KEEP)
    with AUDIT.open(encoding="utf-8", errors="replace") as handle:
        for line in handle:
            try:
                record = json.loads(line)
            except ValueError:
                continue
            outcome = str(record.get("outcome") or record.get("result") or record.get("status") or "")
            allowed = record.get("allowed")
            refused = outcome.lower() in {"deny", "denied", "refused", "error"} or allowed is False
            if refused:
                rows.append(record)
    print(f"refused entries kept: {len(rows)}")
    for record in rows:
        keep = {
            key: record[key]
            for key in ("timestamp", "consumer", "item", "field", "action", "outcome", "reason", "error", "status")
            if key in record
        }
        print(json.dumps(keep, sort_keys=True))
    return len("")


if __name__ == "__main__":
    sys.exit(main())
