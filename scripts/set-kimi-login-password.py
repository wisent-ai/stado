#!/usr/bin/env python3
"""Put the operator's Google password into the Kimi login item, and nowhere else.

The login trajectory reads `kimi-lukasz-google-sso` from Skarbiec: `username`
and `password`, with the method in the item's context. When the operator hands
over a new password it belongs in that item and only there -- not in a launcher,
an environment file, or a command line.

The value arrives as an owner-only file installed by `stado host install-secret`,
is compared by digest so an unchanged password writes nothing, and the file is
removed afterwards. No secret is printed: only digests and lengths.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
ITEM = "kimi-lukasz-google-sso"
FIELD = "password"
DELIVERED = HOME / ".stado" / "kimi-google-password"
ENVIRONMENT = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": VAULT,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
}


def run(*args, stdin=NONE):
    return subprocess.run(
        args, capture_output=True, text=True, input=stdin, check=False, env=ENVIRONMENT
    )


def fingerprint(value):
    return hashlib.sha256(value.encode()).hexdigest()[: len("a" * 12)]


def main():
    if not DELIVERED.is_file():
        raise SystemExit(f"no delivered password at {DELIVERED}")
    fresh = DELIVERED.read_text(encoding="utf-8").strip()
    if not fresh:
        raise SystemExit("the delivered password file is empty")
    proc = run(str(SKARBIEC), "get", ITEM)
    if proc.returncode != ZERO:
        raise SystemExit(f"{ITEM} unreadable: {proc.stderr.strip().splitlines()[-1:]}")
    document = json.loads(proc.stdout)
    fields = document.setdefault("fields", {})
    current = fields.get(FIELD)
    print(f"item       {ITEM}")
    print(f"account    {document.get('context', {}).get('account_ref', '(unnamed)')}")
    print(f"stored     {len(current) if isinstance(current, str) else 0} chars {fingerprint(current) if isinstance(current, str) else '(none)'}")
    print(f"delivered  {len(fresh)} chars {fingerprint(fresh)}")
    if current == fresh:
        print("settled    the item already carries this password")
        DELIVERED.unlink()
        return NONE
    fields[FIELD] = fresh
    written = run(str(SKARBIEC), "set-json", ITEM, stdin=json.dumps(document))
    if written.returncode != ZERO:
        raise SystemExit(f"write refused: {written.stderr.strip().splitlines()[-1:]}")
    DELIVERED.unlink()
    print("updated    password replaced; delivery file removed")
    return NONE


sys.exit(main())
