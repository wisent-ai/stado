#!/usr/bin/env python3
"""Add one field read to an existing Skarbiec consumer without rotating its bearer.

`skarbiec token-mint` writes a whole grant, not a delta: it replaces the stored
capability list and, unless it is handed the current bearer, mints a new one and
keeps only the new hash. For a consumer like `local-operator`, which every Stado
credential delivery authenticates as and which already carries a four-figure
capability list, minting by hand is how a fleet loses its credentials -- one
forgotten capability or one rotated bearer and every delivery starts failing.

So this reads the live grant, refuses unless the consumer's owner-only token file
still hashes to the bearer the vault recorded (a bearer that cannot be reproduced
must not be replaced), takes the union of the existing capabilities with the ones
requested, preserves the remaining TTL, and re-mints with `--token-file` so the
bearer is written back unchanged. The vault is copied first, and the grant is
measured before and after. Running it twice changes nothing.
"""

import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time

NONE = None
ZERO = len([])
ONE = len("a")
HOME = pathlib.Path(os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
VAULT = pathlib.Path(
    os.environ.get("SKARBIEC_VAULT_FILE", str(HOME / ".stado" / "skarbiec.vault.json"))
)
ACTION = "read"
ENVIRONMENT = {
    **os.environ,
    "SKARBIEC_VAULT_FILE": str(VAULT),
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
}


def run(*args):
    return subprocess.run(
        args, capture_output=True, text=True, check=False, env=ENVIRONMENT
    )


def encode(capability):
    field = capability.get("field")
    item = capability["item"]
    return f"{capability['action']}:{item}#{field}" if field else f"{capability['action']}:{item}"


def main():
    if len(sys.argv) != len(["self", "consumer", "item", "field"]):
        raise SystemExit("usage: grant-consumer-field-read.py <consumer> <item> <field>")
    consumer, item, field = sys.argv[ONE], sys.argv[ONE + ONE], sys.argv[ONE + ONE + ONE]
    wanted = {"action": ACTION, "item": item, "field": field}

    document = json.loads(VAULT.read_text(encoding="utf-8"))
    grant = document.get("tokens", {}).get(consumer)
    if grant is NONE:
        raise SystemExit(f"no grant for consumer {consumer}; mint it deliberately first")
    existing = grant["capabilities"]
    remaining = grant["expires_at"] - int(time.time())
    print(f"consumer     {consumer}")
    print(f"capabilities {len(existing)} held")
    print(f"expires_in   {remaining} seconds")
    print(f"requested    {encode(wanted)}")
    if any(capability == wanted for capability in existing):
        print("settled      the consumer already holds this capability; nothing written")
        return NONE
    if remaining <= ZERO:
        raise SystemExit(f"the {consumer} grant expired; re-mint it deliberately instead")

    # A bearer this script cannot reproduce is a bearer it must not replace: the
    # holders of the old one would start failing with no way back.
    #
    # Two spellings, because the fleet has both: `stado-local-agent` keeps its
    # bearer in `local-agent-skarbiec-token`, dropping the product prefix, while
    # `local-operator` and the verifiers carry the consumer name verbatim. The
    # hash check below is what actually decides, so accepting the second spelling
    # loosens nothing: a file that does not hash to the recorded bearer is still
    # refused, and this only stops the script from declaring a bearer absent that
    # is sitting beside the vault under its other name.
    candidates = [HOME / ".stado" / f"{consumer}-skarbiec-token"]
    if consumer.startswith("stado-"):
        candidates.append(
            HOME / ".stado" / f"{consumer[len('stado-'):]}-skarbiec-token"
        )
    present = [candidate for candidate in candidates if candidate.is_file()]
    if not present:
        listed = ", ".join(str(candidate) for candidate in candidates)
        raise SystemExit(f"no bearer file at {listed}; refusing to rotate {consumer}")
    matching = [
        candidate
        for candidate in present
        if hashlib.sha256(candidate.read_text(encoding="utf-8").strip().encode()).hexdigest()
        == grant.get("hash")
    ]
    if not matching:
        listed = ", ".join(str(candidate) for candidate in present)
        raise SystemExit(
            f"{listed} does not hash to the bearer the vault recorded for {consumer}; "
            "refusing to rotate it"
        )
    bearer_file = matching[ZERO]
    bearer = bearer_file.read_text(encoding="utf-8").strip()
    print(f"bearer       {bearer_file.name} matches the recorded hash ({len(bearer)} chars)")

    backup = VAULT.with_suffix(f".before-{consumer}-{field}-grant.json")
    shutil.copy2(VAULT, backup)
    print(f"backup       {backup}")

    union = existing + [wanted]
    minted = run(
        str(SKARBIEC),
        "token-mint",
        consumer,
        "--capabilities",
        ",".join(encode(capability) for capability in union),
        "--token-file",
        str(bearer_file),
        "--replace-capabilities",
        "--ttl-seconds",
        str(remaining),
        "--audience",
        grant.get("audience", consumer),
    )
    if minted.returncode != ZERO:
        raise SystemExit(f"mint refused: {minted.stderr.strip().splitlines()[-1:]}")

    settled = json.loads(VAULT.read_text(encoding="utf-8"))["tokens"][consumer]
    print(f"capabilities {len(settled['capabilities'])} held (was {len(existing)})")
    print(f"bearer       {'unchanged' if settled.get('hash') == grant.get('hash') else 'ROTATED'}")
    print(f"expires_in   {settled['expires_at'] - int(time.time())} seconds")
    print(f"granted      {encode(wanted)}")
    return NONE


sys.exit(main())
