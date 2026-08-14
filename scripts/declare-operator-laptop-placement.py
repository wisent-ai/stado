#!/usr/bin/env python3
"""Declare on the registry what the operator's laptop may not be used for.

`place-by-capability.py` reads measurements, and measurements answer only half
the question. `operator-host` measures `display` true, because someone is
logged in on it -- it is the machine the operator is sitting in front of. A
placement model that reads that as "put the customer browser logins here" is
worse than the habit it replaced: it takes work that used to fail loudly on a
headless host and moves it onto the one screen a person is using.

So the target declares it, once, where every reader can see it:

    "placement": {"excludes": ["display", "browser-render"], "reason": "..."}

Capability ids, the same vocabulary the trajectories declare their needs in, and
exclusions only -- an accepts-allowlist would silently disqualify every host that
has not declared one, which is the same silence this whole model replaces.

Idempotent, and it writes only through `stado registry push`, which validates the
whole document first. It prints the target's policy before and after.
"""

import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = pathlib.Path(os.environ.get("STADO_BIN") or HOME / ".stado" / "bin" / "stado")
TARGET = "operator-host"
POLICY_KEY = "placement"
POLICY = {
    "excludes": ["display", "browser-render"],
    "reason": (
        "operator laptop: product browser journeys never run on the machine the operator is using"
    ),
}
# The document is staged beside the other registry deliveries this repo writes,
# because `registry push` takes a file and a half-written one must never be the
# file it takes.
DELIVERY = HOME / ".stado" / "files" / "registry-operator-laptop-placement.json"


def stado(*args, allow_failure=False):
    proc = subprocess.run(
        [str(STADO), *args], capture_output=True, text=True, check=False
    )
    if proc.returncode != ZERO and not allow_failure:
        raise SystemExit(f"stado {' '.join(args)} failed: {proc.stderr.strip() or proc.stdout}")
    return proc.stdout


def target_of(document, name):
    for entry in document.get("targets", []):
        if entry.get("name") == name:
            return entry
    raise SystemExit(f"{name} is not a target in this registry")


def main():
    document = json.loads(stado("registry", "pull"))
    entry = target_of(document, TARGET)
    before = entry.get(POLICY_KEY)
    print(f"target      {TARGET}")
    print(f"before      {json.dumps(before, sort_keys=True) if before else '(no placement policy)'}")
    if before == POLICY:
        print("settled     the target already declares this policy")
        return NONE

    entry[POLICY_KEY] = POLICY
    body = json.dumps(document, indent=len("ba")) + "\n"
    DELIVERY.parent.mkdir(parents=True, exist_ok=True)
    DELIVERY.write_text(body, encoding="utf-8")
    print(stado("registry", "validate", str(DELIVERY)).strip())
    print(stado("registry", "push", str(DELIVERY)).strip())

    # Read it back out of the canonical store rather than trusting the push: a
    # write that lost a concurrent update is a write that pushed a stale copy of
    # everyone else's declarations too.
    settled = target_of(json.loads(stado("registry", "pull")), TARGET).get(POLICY_KEY)
    print(f"after       {json.dumps(settled, sort_keys=True)}")
    print(f"delivery    {DELIVERY}")
    return NONE


sys.exit(main())
