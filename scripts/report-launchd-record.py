#!/usr/bin/env python3
"""Print launchd's own record for the resolver job, unparsed.

Every verdict about whether a service is supervised has been derived from a
regex over this output, and two of those verdicts were wrong. Print the head of
the record itself so the state, pid and exit history are read rather than
inferred.

Read-only.
"""

import os
import subprocess
import sys

NONE = None
LABEL = os.environ.get("STADO_RESOLVER_LABEL", "com.wisent.stado-resolver")
LINES = len("l" * 30)


def main():
    proc = subprocess.run(
        ["/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}"],
        capture_output=True,
        text=True,
        check=False,
    )
    text = (proc.stdout or "") + (proc.stderr or "")
    for line in text.splitlines()[:LINES]:
        print(line)
    return NONE


sys.exit(main())
