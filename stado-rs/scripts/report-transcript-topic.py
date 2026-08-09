#!/usr/bin/env python3
"""Print user turns and assistant messages about one topic, newest first.

Answers "what did we last say about X" from the stored record rather than from
memory. Two export shapes are read: the Oko lake export, which carries a flat
`text` with an `event_type`, and raw runtime logs, which nest
`message.content` as a string or a list of parts.

Usage: report-transcript-topic.py <root> <term> [<term> ...]
"""
from __future__ import annotations

import json
import os
import sys


def texts(row):
    if isinstance(row.get("text"), str) and row["text"]:
        return [row["text"]]
    message = row.get("message") if isinstance(row.get("message"), dict) else row
    content = message.get("content")
    if isinstance(content, str):
        return [content]
    if isinstance(content, list):
        return [
            part["text"]
            for part in content
            if isinstance(part, dict) and isinstance(part.get("text"), str)
        ]
    return []


def role_of(row):
    role = row.get("event_type")
    if role in ("user", "assistant"):
        return role
    message = row.get("message") if isinstance(row.get("message"), dict) else row
    return message.get("role") or row.get("role")


def main() -> int:
    root, terms = sys.argv[1], [t.casefold() for t in sys.argv[2:]]
    hits = []
    for base, _dirs, files in os.walk(root):
        for name in files:
            if not name.endswith(".jsonl"):
                continue
            path = os.path.join(base, name)
            try:
                with open(path, errors="replace") as handle:
                    for line in handle:
                        if not all(term in line.casefold() for term in terms):
                            continue
                        try:
                            row = json.loads(line)
                        except ValueError:
                            continue
                        if role_of(row) not in ("user", "assistant"):
                            continue
                        for text in texts(row):
                            low = text.casefold()
                            if all(term in low for term in terms):
                                stamp = row.get("ts") or row.get("timestamp") or ""
                                hits.append((stamp, role_of(row), path, text))
            except OSError:
                continue
    hits.sort(reverse=True)
    for stamp, role, path, text in hits:
        print("=" * 90)
        print(f"{stamp}  {role}  {os.path.basename(path)[:16]}")
        print(text.strip()[:1500])
    print(f"total matches: {len(hits)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
