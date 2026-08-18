#!/usr/bin/env python3
"""Re-apply the target facts a stale-copy push overwrote.

A push built from this laptop's LOCAL copy of the store landed on the canonical
object and silently reverted two edits that had been made against the canonical
document: the machine-account reference on the authority host, and the retirement
of a Weles service declaration on the operator laptop that nothing there runs.
Compare-and-swap protected the object's version, not the freshness of the
document behind it -- which is exactly why an edit must be read from the store it
will be written to.

This re-applies both facts on top of whatever the store holds now, advances the
service-directory generation because readers refuse a directory that changed
without one, and refuses to run against a store that is not the canonical one.
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
CONFIG = pathlib.Path(os.environ.get("STADO_CONFIG", HOME / ".config" / "stado" / "config.json"))
STAGED = HOME / ".stado" / "registry-repair-lost-update.json"
AUTHORITY_ACCOUNT = ("charless-mac-mini", "host-account-charless-mac-mini")
UNTRUE_SERVICE = ("lukasz-macbook", "com.wisent.always-on.weles")
DROP_ENDPOINT = ("stado-object-api", "lukasz-macbook")


def run(*args):
    return subprocess.run(args, capture_output=True, text=True, check=False)


def canonical_url():
    document = json.loads(CONFIG.read_text(encoding="utf-8")) if CONFIG.is_file() else {}
    return ((document.get("storage") or {}).get("stado") or {}).get("url", "")


def main():
    if os.environ.get("WC_STADO_STORAGE_URL"):
        raise SystemExit(
            "WC_STADO_STORAGE_URL is set: this edit must read and write the canonical store, "
            "not an override that may be a local copy"
        )
    print(f"store      {canonical_url() or '(unset)'}")
    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {(pulled.stderr or pulled.stdout).strip()[:160]}")
    document = json.loads(pulled.stdout)
    changed = []
    for entry in document.get("targets", []):
        name = entry.get("name")
        if name == AUTHORITY_ACCOUNT[ZERO] and entry.get("account_ref") != AUTHORITY_ACCOUNT[len("a")]:
            entry["account_ref"] = AUTHORITY_ACCOUNT[len("a")]
            changed.append(f"{name}: account_ref restored")
        if name == UNTRUE_SERVICE[ZERO]:
            services = entry.get("services") or []
            kept = [
                item
                for item in services
                if UNTRUE_SERVICE[len("a")] not in (item.get("name"), item.get("label"))
            ]
            if len(kept) != len(services):
                entry["services"] = kept
                changed.append(f"{name}: {UNTRUE_SERVICE[len('a')]} retired again")
    directory = document.get("service_directory") or {}
    endpoints = ((directory.get("services") or {}).get(DROP_ENDPOINT[ZERO]) or {}).get("endpoints") or {}
    if DROP_ENDPOINT[len("a")] in endpoints:
        # The fleet store lives on the authority. Advertising the laptop's own
        # object API as the same service is what let a client address and a
        # service address be confused for each other.
        endpoints.pop(DROP_ENDPOINT[len("a")])
        changed.append(f"{DROP_ENDPOINT[ZERO]}: endpoint for {DROP_ENDPOINT[len('a')]} withdrawn")
    if not changed:
        print("settled    the canonical document already carries every fact")
        return NONE
    generation = directory.get("generation")
    if isinstance(generation, int):
        directory["generation"] = generation + len("a")
        print(f"generation {generation} -> {directory['generation']}")
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
    for item in changed:
        print(f"repaired   {item}")
    return NONE


sys.exit(main())
