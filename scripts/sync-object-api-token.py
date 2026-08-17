#!/usr/bin/env python3
"""Bring this host's object-API bearer back in line with the vault.

The token file on an operator machine is a copy, and copies drift: reads that
worked this morning answered 401 this evening while the authority host, reading
with its own copy, answered 200. A drifted bearer is indistinguishable from a
revoked grant or a policy change until someone compares the two, which is what
this does -- by digest, never by value.

Writes only when the digests differ, keeps the previous file beside it with a
timestamp, and prints digests and lengths.
"""

import datetime
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
ITEM = os.environ.get("OBJECT_API_ITEM", "probierz-object-api")
FIELD = os.environ.get("OBJECT_API_FIELD", "token")
TARGET = pathlib.Path(os.environ.get("OBJECT_API_TOKEN_FILE", HOME / ".stado" / "wisent-queue-object-api-token"))


def fingerprint(text):
    return hashlib.sha256(text.encode()).hexdigest()[: len("a" * 16)]


def main():
    proc = subprocess.run(
        [str(SKARBIEC), "get", ITEM],
        capture_output=True,
        text=True,
        check=False,
        env={
            **os.environ,
            "SKARBIEC_VAULT_FILE": VAULT,
            # The vault is GPG-backed and the agent's PATH does not carry
            # Homebrew, so a helper reading an item fails with `spawn gpg` on a
            # host where the CLI works fine by hand.
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
        },
    )
    if proc.returncode != ZERO:
        raise SystemExit(f"{ITEM} unreadable: {(proc.stderr or proc.stdout).strip()[: len('a' * 120)]}")
    document = json.loads(proc.stdout)
    fields = document.get("fields") or {}
    candidates = [name for name in (FIELD, "bearer", "value", "api_token") if isinstance(fields.get(name), str)]
    if not candidates:
        raise SystemExit(f"{ITEM} carries no token field; fields present: {sorted(fields)}")
    fresh = fields[candidates[ZERO]].strip()
    print(f"item       {ITEM} field {candidates[ZERO]} {len(fresh)} chars {fingerprint(fresh)}")
    current = TARGET.read_text(encoding="utf-8").strip() if TARGET.is_file() else ""
    print(f"file       {TARGET} {len(current)} chars {fingerprint(current) if current else '(absent)'}")
    if current == fresh:
        print("settled    the file already carries the vault's bearer")
        return NONE
    if TARGET.is_file():
        stamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%SZ")
        backup = TARGET.with_name(f"{TARGET.name}.before-{stamp}")
        shutil.copy2(TARGET, backup)
        print(f"backup     {backup}")
    os.umask(0o177)
    TARGET.write_text(fresh + "\n", encoding="utf-8")
    os.chmod(TARGET, 0o600)
    print(f"updated    {TARGET} now carries {fingerprint(fresh)}")
    return NONE


sys.exit(main())
