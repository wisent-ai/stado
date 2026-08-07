#!/usr/bin/env python3
"""Print the tail of the Skarbiec audit as-is, so its own field names are visible.

`skarbiec-recent-refusals.py` guessed at the outcome field and found nothing.
Before guessing again, read what the server actually records.
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
    rows: deque[str] = deque(maxlen=KEEP)
    with AUDIT.open(encoding="utf-8", errors="replace") as handle:
        for line in handle:
            rows.append(line.rstrip())
    keys: set[str] = set()
    for line in rows:
        try:
            keys.update(json.loads(line).keys())
        except ValueError:
            continue
    print("field names seen:", ",".join(sorted(keys)))
    for line in rows:
        print(line[: len("x" * len("-" * len("----------------------------------------")) * len("xxxxx"))])
    return len("")


if __name__ == "__main__":
    sys.exit(main())
