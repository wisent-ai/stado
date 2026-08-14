#!/usr/bin/env python3
"""Read what the last login run recorded: its own steps, then the page it died on.

The trajectory writes a child log, a DOM snapshot and a video. The log says which
step it reached; the DOM says what the page was showing when it stopped. Printing
the step trail and the page's visible text is what turns "no consent/code" into a
statement about the screen.

Read-only, and it prints page text: a Google login screen carries an email
address, so the account is shown and nothing else is.
"""

import html
import os
import pathlib
import re
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
RELEASE = HOME / ".local" / "share" / "weles-worker" / "0.5.8" / "darwin-arm64"
CHILD_LOGS = (
    RELEASE / "var" / "kimi-login-child-last.log",
    RELEASE / "var" / "claude-login-child-last.log",
)
RECORDINGS = RELEASE / "recordings" / "local"
TAIL = len("l" * 30)
TEXT_LIMIT = len("t" * 1600)


def visible_text(markup):
    body = re.sub(r"(?is)<(script|style|svg|noscript)[^>]*>.*?</\1>", " ", markup)
    body = re.sub(r"(?s)<[^>]+>", " ", body)
    return re.sub(r"\s+", " ", html.unescape(body)).strip()


def main():
    for path in CHILD_LOGS:
        print(f"== {path.name} {'present' if path.is_file() else 'absent'}")
        if not path.is_file():
            continue
        lines = [line.rstrip() for line in path.read_text(encoding="utf-8", errors="replace").splitlines() if line.strip()]
        for line in lines[-TAIL:]:
            print(f"  {line[: len('a' * 180)]}")

    snapshots = sorted(RECORDINGS.rglob("session_dom_*.html"), key=lambda p: p.stat().st_mtime)
    if not snapshots:
        print("no DOM snapshot recorded")
        return NONE
    latest = snapshots[-len(["newest"])]
    print(f"== {latest} ({latest.stat().st_size} bytes)")
    markup = latest.read_text(encoding="utf-8", errors="replace")
    title = re.search(r"(?is)<title[^>]*>(.*?)</title>", markup)
    print(f"  title {html.unescape(title.group(len(['t'])).strip()) if title else '(none)'}")
    for pattern in (r'data-url="([^"]+)"', r'<base href="([^"]+)"', r'"currentUrl"\s*:\s*"([^"]+)"'):
        found = re.search(pattern, markup)
        if found:
            print(f"  url {found.group(len(['u']))}")
            break
    print(f"  text {visible_text(markup)[:TEXT_LIMIT]}")
    return NONE


sys.exit(main())
