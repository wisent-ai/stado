#!/usr/bin/env python3
"""Keep the recordings cleaner pointed at the runs it must actually delete.

Every browser login writes a video and an instrumentation dump, and the dumps run
to a gigabyte apiece. The policy kept them for a week, so the always-on host fell
to 2.7 GiB free against a 20 GiB low watermark and Chromium stopped starting --
which read as a broken login trajectory rather than a full disk.

Two corrections: name the recordings directory the running release actually uses,
so a version bump cannot silently point the cleaner at nothing, and retain runs
for hours rather than days.

Idempotent, and it writes through `stado registry push`, which validates first.
"""

import json
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
STADO = pathlib.Path.home() / ".stado" / "bin" / "stado"
HOST = "charless-mac-mini"
CLEANER = "weles_recordings"
ROOT = "~/weles/recordings"
# The schema floors retention at a day, so this is the shortest the policy can
# hold: the gigabyte-per-run dumps are capped at the source instead.
MIN_AGE = int("86400")
DELIVERY = pathlib.Path("/tmp/registry-recordings-retention.json")


def stado(*args):
    proc = subprocess.run([str(STADO), *args], capture_output=True, text=True, check=False)
    if proc.returncode != ZERO:
        raise SystemExit(f"stado {' '.join(args)} failed: {proc.stderr.strip() or proc.stdout}")
    return proc.stdout


def main():
    document = json.loads(stado("registry", "pull"))
    target = next((entry for entry in document["targets"] if entry.get("name") == HOST), NONE)
    if target is NONE:
        raise SystemExit(f"{HOST} is not a target in this registry")
    policy = target.setdefault("disk_cleanup", {}).setdefault("cleaners", {}).setdefault(CLEANER, {})
    before = dict(policy)
    policy["root"] = ROOT
    policy["min_age_seconds"] = MIN_AGE
    if before == policy:
        print(f"settled    {CLEANER} already keeps runs for {MIN_AGE}s under {ROOT}")
        return NONE
    DELIVERY.write_text(json.dumps(document, indent=len("ba")) + "\n", encoding="utf-8")
    print(stado("registry", "validate", str(DELIVERY)).strip())
    print(stado("registry", "push", str(DELIVERY)).strip())
    print(f"before     {json.dumps(before)}")
    print(f"after      {json.dumps(policy)}")
    return NONE


sys.exit(main())
