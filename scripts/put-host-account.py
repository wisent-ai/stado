#!/usr/bin/env python3
"""Put a fleet host's operating-system account into Skarbiec, and nowhere else.

An operator hands over a machine account once, usually in a chat window. A chat
window is not a credential store: the next agent cannot read it, the fleet cannot
read it, and it survives in transcripts that nothing rotates. The account belongs
in Skarbiec as a `host-account` item -- a kind of its own, because a machine
account and a web `login` have different readers and a login trajectory that
iterates `login` items must never find a host root account among them.

The password arrives as an owner-only file (`stado host install-secret`, or an
umask 177 write on the vault host) so it never appears in argv, is handed to
`skarbiec set-json` on stdin, and the delivery file is removed afterwards. The
item id is not guessed: it comes from the registry target's `account_ref`, which
is the fleet's only pointer from a host name to its account. Nothing secret is
printed -- only digests and lengths.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
ONE = len("a")
HOME = pathlib.Path(os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
STADO = HOME / ".stado" / "bin" / "stado"
VAULT = os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
KIND = "host-account"
SCHEMA = "skarbiec.item.v2"
SOURCE_KIND = "fleet-host"
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
    if not isinstance(value, str):
        return "(none)"
    return hashlib.sha256(value.encode()).hexdigest()[: len("a" * 12)]


def measured(value):
    return f"{len(value) if isinstance(value, str) else ZERO} chars {fingerprint(value)}"


def declared_item(target):
    """The item id the registry says holds this host's account."""
    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {pulled.stderr.strip().splitlines()[-1:]}")
    for entry in json.loads(pulled.stdout).get("targets", []):
        if entry.get("name") == target:
            reference = entry.get("account_ref")
            if not reference:
                raise SystemExit(
                    f"registry target {target} declares no account_ref; "
                    "run declare-host-account-ref.py first so the pointer exists"
                )
            return reference
    raise SystemExit(f"no registry target named {target}")


def main():
    if len(sys.argv) != len(["self", "target", "username"]):
        raise SystemExit("usage: put-host-account.py <registry-target> <account-username>")
    target, username = sys.argv[ONE], sys.argv[ONE + ONE]
    item = declared_item(target)
    delivered = HOME / ".stado" / f"{item}-password"

    if not delivered.is_file():
        raise SystemExit(f"no delivered password at {delivered}")
    if delivered.stat().st_mode & 0o077:
        raise SystemExit(f"{delivered} is not owner-only; refusing to read it")
    fresh = delivered.read_text(encoding="utf-8").strip()
    if not fresh:
        raise SystemExit("the delivered password file is empty")

    # An absent item is the normal first run, so a failed read is not an error
    # here; only a read that returns something unusable is.
    existing = run(str(SKARBIEC), "get", item)
    document = json.loads(existing.stdout) if existing.returncode == ZERO else {}
    fields = dict(document.get("fields", {}))
    account = f"{username}@{target}"

    print(f"target       {target}")
    print(f"item         {item}  (kind {KIND})")
    print(f"account_ref  {account}")
    print(f"present      {'yes' if document else 'no'}")
    print(f"username     stored {fields.get('username') or '(none)'}, requested {username}")
    print(f"stored       {measured(fields.get('password'))}")
    print(f"delivered    {measured(fresh)}")

    if fields.get("username") == username and fields.get("password") == fresh:
        print("settled      the vault already carries this account; nothing written")
        delivered.unlink()
        return NONE

    payload = {
        "schema": SCHEMA,
        "kind": KIND,
        "fields": {"username": username, "password": fresh},
        "context": {"account_ref": account, "source_kind": SOURCE_KIND},
    }
    # Tags, not the id, are how a consumer finds its own items: the id is opaque
    # and a rename would silently drop the item out of every listing.
    written = run(
        str(SKARBIEC),
        "set-json",
        item,
        "--tags",
        f"fleet:host-account,fleet:target:{target}",
        stdin=json.dumps(payload),
    )
    if written.returncode != ZERO:
        raise SystemExit(f"write refused: {written.stderr.strip().splitlines()[-1:]}")
    delivered.unlink()
    # `set-json` acknowledges with {"ok","id","kind"} and no revision, so the
    # count comes from the envelope the vault kept rather than from the request.
    envelope = next(
        (
            entry
            for entry in json.loads(run(str(SKARBIEC), "list").stdout)
            if entry.get("id") == item
        ),
        {},
    )
    print(f"written      {written.stdout.strip()}")
    print(f"envelope     revision {envelope.get('revision', '?')} tags {envelope.get('tags', [])}")
    print(f"removed      {delivered}")
    return NONE


sys.exit(main())
