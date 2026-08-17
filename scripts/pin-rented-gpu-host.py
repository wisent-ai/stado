#!/usr/bin/env python3
"""Stop the rented GPU host from auto-claiming queue backlog while its renter gate is blind.

`stado agent` pauses fleet claims when a Vast.ai renter is active, and on
2026-08-17 that gate could not evaluate on the machine it protects:

    [vast] cannot read stado-vast/api_key from Skarbiec: cannot read Skarbiec
    grant file /root/.stado/control-plane-skarbiec-token: No such file

The Vast key is read through the *control-plane* credential channel
(`secrets.skarbiec.*`, consumer `stado-control-plane`), which a worker host does
not hold and should not: the host's own channel is `agent.skarbiec.*`, consumer
`stado-local-agent`, whose bearer is installed and whose item list does not carry
`stado-vast`. So `vast_active` is permanently false there, and nothing stops a
queued job from landing on a board a paying renter is using -- one was, at 98%
utilisation and 60 GiB, while this was written.

`ComputeTarget.pinned_only` is the declaration for exactly this shape ("keeps
shared workstations from picking up stray queue backlog"), it has a typed reader,
and it leaves deliberate placement intact: a job pinned to this host still runs.

Reversible in one command:

    RETIRE_PIN=1 python3 scripts/pin-rented-gpu-host.py

Compare-and-swap safe: the document is read, changed, and pushed; a concurrent
writer makes the push fail rather than silently win.
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
TARGET = os.environ.get("PIN_TARGET", "gpu-host")
DESIRED = not os.environ.get("RETIRE_PIN")
STAGED = HOME / ".stado" / "registry-pin-rented-gpu-host.json"


def run(*args, stdin=NONE):
    return subprocess.run(args, capture_output=True, text=True, input=stdin, check=False)


def main():
    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {(pulled.stderr or pulled.stdout).strip()[:200]}")
    document = json.loads(pulled.stdout)
    changed = NONE
    for entry in document.get("targets", []):
        if entry.get("name") != TARGET:
            continue
        current = bool(entry.get("pinned_only"))
        if current == DESIRED:
            print(f"settled    {TARGET} already declares pinned_only={current}")
            return NONE
        entry["pinned_only"] = DESIRED
        changed = f"pinned_only {current} -> {DESIRED}"
    if changed is NONE:
        raise SystemExit(f"registry has no target named {TARGET}")
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
    print(f"declared   {TARGET}: {changed}")
    return NONE


sys.exit(main())
