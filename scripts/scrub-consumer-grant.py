#!/usr/bin/env python3
"""Drop a consumer's capabilities that name items the vault no longer holds.

Grants are additive: `grant-consumer-field-read.py` unions, `token-mint`
replaces, and nothing subtracts. So when a credential item is deleted and
purged, every capability that named it stays in the grant — and the next
widening re-mint is refused outright with `capability names a missing item`.
One removed SSH key then blocks every future grant for that consumer, which
is how a deleted test key froze `local-operator` on the operator's laptop.

This re-mints the grant with the same list MINUS capabilities whose item is
absent from the vault (not present, or present only in trash — a trashed item
blocks minting too, and keeping its capability would re-freeze the grant the
moment someone purges it). Everything else is the proven mechanics of
`grant-consumer-field-read.py`: refuse unless the consumer's token file hashes
to the recorded bearer, re-mint with `--token-file` so the bearer survives,
preserve the remaining TTL, copy the vault first, change nothing when there is
nothing to drop.

Usage: scrub-consumer-grant.py <consumer>
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
    if len(sys.argv) != len(["self", "consumer"]):
        raise SystemExit("usage: scrub-consumer-grant.py <consumer>")
    consumer = sys.argv[ONE]

    document = json.loads(VAULT.read_text(encoding="utf-8"))
    grant = document.get("tokens", {}).get(consumer)
    if grant is NONE:
        raise SystemExit(f"no grant for consumer {consumer}; nothing to scrub")
    existing = grant["capabilities"]
    remaining = grant["expires_at"] - int(time.time())
    if remaining <= ZERO:
        raise SystemExit(f"the {consumer} grant expired; re-mint it deliberately instead")

    # An item present only in trash still blocks minting, so for this purpose
    # it is as gone as a purged one. `skarbiec list --deleted` shows both.
    listed = run(str(SKARBIEC), "list", "--deleted")
    if listed.returncode != ZERO:
        raise SystemExit(f"cannot list the vault: {listed.stderr.strip()}")
    live = {
        row["id"]
        for row in json.loads(listed.stdout)
        if not row.get("deleted", False)
    }

    kept, dropped = [], []
    for capability in existing:
        (kept if capability["item"] in live else dropped).append(capability)
    print(f"consumer     {consumer}")
    print(f"capabilities {len(existing)} held, {len(dropped)} name missing items")
    for capability in dropped:
        print(f"  dropping   {encode(capability)}")
    if not dropped:
        print("settled      every capability names a live item; nothing written")
        return NONE

    candidates = [HOME / ".stado" / f"{consumer}-skarbiec-token"]
    if consumer.startswith("stado-"):
        candidates.append(HOME / ".stado" / f"{consumer[len('stado-'):]}-skarbiec-token")
    present = [candidate for candidate in candidates if candidate.is_file()]
    if not present:
        names = ", ".join(str(candidate) for candidate in candidates)
        raise SystemExit(f"no bearer file at {names}; refusing to rotate {consumer}")
    matching = [
        candidate
        for candidate in present
        if hashlib.sha256(candidate.read_text(encoding="utf-8").strip().encode()).hexdigest()
        == grant.get("hash")
    ]
    if not matching:
        names = ", ".join(str(candidate) for candidate in present)
        raise SystemExit(
            f"{names} does not hash to the bearer the vault recorded for {consumer}; "
            "refusing to rotate it"
        )
    bearer_file = matching[ZERO]

    backup = VAULT.with_suffix(f".before-{consumer}-scrub.json")
    shutil.copy2(VAULT, backup)
    print(f"backup       {backup}")

    minted = run(
        str(SKARBIEC),
        "token-mint",
        consumer,
        "--capabilities",
        ",".join(encode(capability) for capability in kept),
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
    return NONE


sys.exit(main())
