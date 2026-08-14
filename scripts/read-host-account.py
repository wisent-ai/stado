#!/usr/bin/env python3
"""Confirm that a fleet host's operating-system account exists, without exposing it.

Given a registry target name this follows the only pointer the fleet has -- the
target's `account_ref` -- into Skarbiec, and reports what is there: the account
username, the length and digest of the password, the tags a consumer can find the
item by, and which registered consumers hold a capability on it. The password
itself is never printed; the redacted document at the end is the paste-safe proof
that both fields are present.

The point is that a later agent never has to ask a human for this credential
again, and never has to read it to know it is there. When the value really is
needed, the reader is Stado's own consumer path -- `stado host install-credential`
delivers one exact field to one host as an owner-only file and prints nothing --
so this script names that command instead of becoming a way to print a secret.

Exits non-zero when the declaration and the world disagree: a target that names
an item nothing holds, or an item whose context names a different host, is the
same failure shape as a registry field with no reader.
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
SECRET_FIELD = "password"
ENVIRONMENT = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": VAULT,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
}


def run(*args):
    return subprocess.run(
        args, capture_output=True, text=True, check=False, env=ENVIRONMENT
    )


def digest(value):
    return "sha256:" + hashlib.sha256(value.encode()).hexdigest()[: len("a" * 12)]


def redacted(document):
    """The item as stored, with every secret field replaced by its measurement."""
    copy = json.loads(json.dumps(document))
    for name, value in copy.get("fields", {}).items():
        if name == "username" or not isinstance(value, str):
            continue
        copy["fields"][name] = f"<redacted {len(value)} chars {digest(value)}>"
    return copy


def main():
    if len(sys.argv) > len(["self", "target"]):
        raise SystemExit("usage: read-host-account.py [registry-target]")
    # A Stado helper is run with no arguments, so with none this asks about the
    # host it is running on -- which is the question a host asks about itself.
    if len(sys.argv) == len(["self"]):
        resolved = run(str(STADO), "registry", "self")
        if resolved.returncode != ZERO:
            raise SystemExit(
                f"no target given and `registry self` failed: "
                f"{resolved.stderr.strip().splitlines()[-ONE:]}"
            )
        # `registry self` answers `<name>\t<kind>\t<hostname>`; the target name is
        # the first column, and the hostname is deliberately not it.
        target = resolved.stdout.strip().splitlines()[-ONE].split("\t")[ZERO]
    else:
        target = sys.argv[ONE]
    faults = []

    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {pulled.stderr.strip().splitlines()[-1:]}")
    declaration = next(
        (
            entry
            for entry in json.loads(pulled.stdout).get("targets", [])
            if entry.get("name") == target
        ),
        NONE,
    )
    if declaration is NONE:
        raise SystemExit(f"no registry target named {target}")
    item = declaration.get("account_ref")
    print(f"target       {target}")
    print(f"account_ref  {item or '(absent)'}   (registry target field)")
    if not item:
        print(f"fault        {target} declares no account_ref, so no account can be found")
        return ONE

    # Where the credential must NOT be is as load-bearing as where it must be. A
    # host-account exists so an operator or a repair path can authenticate TO a
    # host, so it belongs in the vault of whoever does that -- never in the vault
    # of the machine it opens, where compromising the host would hand over the
    # host's own admin account.
    here = run(str(STADO), "registry", "self")
    myself = (
        here.stdout.strip().splitlines()[-ONE].split("\t")[ZERO]
        if here.returncode == ZERO and here.stdout.strip()
        else ""
    )
    opens_this_host = myself == target
    envelope = next(
        (
            entry
            for entry in json.loads(run(str(SKARBIEC), "list").stdout)
            if entry.get("id") == item
        ),
        NONE,
    )
    if envelope is NONE:
        if opens_this_host:
            print(f"posture      correct: {target} does not keep the account that opens it")
            print(f"read it on   any host that authenticates to {target}, not on {target}")
            return NONE
        print(f"fault        the vault holds no item {item}; the declaration has no reader")
        return ONE
    if opens_this_host:
        faults.append(
            f"{target} holds {item}, the account that opens {target}; taking the host "
            "would then hand over the host's own admin account"
        )
    print(f"item         {item}")
    print(f"kind         {envelope.get('kind')} (expected {KIND})")
    print(f"revision     {envelope.get('revision')} updated {envelope.get('updated_at')}")
    print(f"tags         {','.join(envelope.get('tags', [])) or '(none)'}")
    if envelope.get("kind") != KIND:
        faults.append(f"item {item} is kind {envelope.get('kind')}, not {KIND}")
    if envelope.get("deleted"):
        faults.append(f"item {item} is in the trash")

    read = run(str(SKARBIEC), "get", item)
    if read.returncode != ZERO:
        print(f"fault        {item} is unreadable here: {read.stderr.strip().splitlines()[-1:]}")
        return ONE
    document = json.loads(read.stdout)
    fields = document.get("fields", {})
    context = document.get("context", {})
    account = context.get("account_ref", "")
    print(f"context      {account or '(unnamed)'} source_kind {context.get('source_kind', '(none)')}")
    print(f"username     {fields.get('username') or '(absent)'}")
    secret = fields.get(SECRET_FIELD)
    if isinstance(secret, str) and secret:
        print(f"{SECRET_FIELD}     {len(secret)} chars {digest(secret)}")
    else:
        print(f"{SECRET_FIELD}     (absent)")
        faults.append(f"item {item} carries no {SECRET_FIELD}")
    # A credential naming a different host than the target that points at it is
    # the contradiction this whole convention exists to make visible.
    if account and not account.endswith(f"@{target}"):
        faults.append(f"context account_ref {account} does not name host {target}")
    if account and fields.get("username") and account != f"{fields['username']}@{target}":
        faults.append(f"context account_ref {account} disagrees with username {fields['username']}")

    grants = [
        f"{grant['consumer']}:{capability['action']}"
        for grant in json.loads(run(str(SKARBIEC), "tokens").stdout)
        for capability in grant.get("capabilities", [])
        if capability.get("item") == item
    ]
    print(f"vault        {VAULT}")
    print(f"consumers    {','.join(grants) or '(none registered)'}")
    # `stado credentials ls` lists only what the credential-store admin consumer
    # holds a capability on, so an item's presence there is a measurement of the
    # grant rather than of the vault: until a consumer held a capability on it this
    # item was one of the thirty-two the catalogue could not see.
    catalogue = run(str(STADO), "credentials", "ls")
    listed = any(
        line.split()[ZERO] == item
        for line in catalogue.stdout.splitlines()[ONE + ONE :]
        if line.split()
    )
    print(f"catalogue    stado credentials ls {'lists' if listed else 'does NOT list'} {item}")
    print(
        f"delivery     stado host install-credential <host> {item} {SECRET_FIELD} <basename>"
    )
    # The delivery command does not read this file: it asks the Skarbiec service
    # named in Stado's config, so that process has to have the grant loaded. On a
    # long-running `skarbiec serve` a grant minted afterwards is not in effect yet.
    print("             (authenticates through the Skarbiec service, which must hold this grant)")
    print("document     (redacted; the value is never printed by this script)")
    print(json.dumps(redacted(document), indent=len("  "), sort_keys=True))

    for fault in faults:
        print(f"fault        {fault}")
    return ONE if faults else NONE


sys.exit(main())
