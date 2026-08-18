#!/usr/bin/env python3
"""Advance the service directory's generation to match its content.

A resolver refuses a directory whose content changed while its generation stood
still: it cannot tell a legitimate edit from a stale cache, and answering either
way would be a guess. That refusal took the fleet's read path down three times
today, every time after a correct registry edit -- because `registry push` does
not touch `service_directory.generation`, so every writer must.

Run on the authority, where the push does not travel through the adapter that the
stale generation just disabled. Reads the host's own registry, advances the
generation by one, validates, and pushes. Prints both numbers so the move is
visible in the log that recorded the outage.
"""

import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
STAGED = HOME / ".stado" / "registry-generation-bump.json"


def run(*args):
    return subprocess.run(args, capture_output=True, text=True, check=False)


def main():
    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {(pulled.stderr or pulled.stdout).strip()[:160]}")
    document = json.loads(pulled.stdout)
    directory = document.get("service_directory")
    if not isinstance(directory, dict):
        raise SystemExit("this registry has no service_directory to advance")
    before = directory.get("generation")
    if not isinstance(before, int):
        raise SystemExit(f"generation is not a number: {before!r}")
    directory["generation"] = before + 1
    STAGED.parent.mkdir(parents=True, exist_ok=True)
    STAGED.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    validated = run(str(STADO), "registry", "validate", str(STAGED))
    print(f"validate   {(validated.stdout or validated.stderr).strip().splitlines()[-1:]}")
    if validated.returncode != ZERO:
        raise SystemExit("the edited registry does not validate; nothing was pushed")
    pushed = run(str(STADO), "registry", "push", str(STAGED))
    print(f"push       {(pushed.stdout or pushed.stderr).strip().splitlines()[-1:]}")
    if pushed.returncode != ZERO:
        raise SystemExit("push refused; the canonical document is unchanged")
    STAGED.unlink()
    print(f"generation {before} -> {before + 1}")
    return NONE


sys.exit(main())
