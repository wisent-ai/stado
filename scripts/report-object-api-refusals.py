#!/usr/bin/env python3
"""Show the object API's own words for its recent refusals.

A 401 that appears for one caller and not another is a decision the service made
and logged. Reading that line is the difference between fixing the cause and
guessing at tokens: the same URI answered 200 on the authority host's loopback
and 401 through the operator's adapter within the same minute.

Read-only. Prints matching log lines, newest last, with the query strings intact
because they name the object, not a secret.
"""

import os
import pathlib
import re
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LOG = pathlib.Path(
    os.environ.get(
        "OBJECT_API_LOG", HOME / ".stado" / "logs" / "com.wisent.always-on.stado-object-api.log"
    )
)
INTERESTING = re.compile(r"(401|403|unauthorized|forbidden|authorization|namespace|prefix|denied)", re.IGNORECASE)
TAIL = 4000
SHOW = len("a" * 25)


def main():
    if not LOG.is_file():
        raise SystemExit(f"no log at {LOG}")
    lines = LOG.read_text(encoding="utf-8", errors="replace").splitlines()[-TAIL:]
    picked = [line.rstrip() for line in lines if INTERESTING.search(line)]
    print(f"log {LOG} ({len(lines)} recent lines, {len(picked)} about authorization)")
    for line in picked[-SHOW:]:
        print(f"   {line[: len('a' * 165)]}")
    if not picked:
        for line in lines[-len("a" * 10):]:
            print(f"   {line[: len('a' * 165)]}")
    return NONE


sys.exit(main())
