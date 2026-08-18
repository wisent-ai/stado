#!/usr/bin/env python3
"""Re-mint one consumer's grant onto a bearer the operator supplies, adding capabilities.

`grant-consumer-field-read.py` is the safe path and should be tried first: it adds
a capability while keeping the bearer, and it refuses unless the recorded bearer
can be reproduced from an owner-only file beside the vault. On 2026-08-17 it
refused for `stado-local-agent`, correctly: the only file with that name here is a
stale copy that does not hash to what the vault recorded, because the live bearer
sits on the worker host and the fleet has no way to move a bearer back to the
vault machine.

So this is the other operation, named for what it does. It replaces the bearer
with one supplied in a file — `skarbiec token-mint --token-file` stores exactly
that string — and takes the union of the existing capabilities with the ones
requested. Everything the vault recorded otherwise is preserved: audience, and
the remaining TTL rather than a fresh one.

The cost is explicit: every holder of the old bearer stops authenticating the
moment this runs, so the caller must deliver the new file to each of them
(`stado host install-secret`). It refuses when it cannot see how many holders
there are, which is why the holder count is an argument and not a guess: pass the
number you verified, and deliver to every one of them in the same operation.

Measure that count from configuration, not from one convention. `stado-local-agent`
has two holders and they declare it differently: the RTX host through
`WC_AGENT_SKARBIEC_TOKEN_FILE` in an env file, the mac mini through
`agent.skarbiec.token_file` in `~/.config/stado/config.json`. A probe that looked
only for env files reported one holder and would have taken the other's
credential reads down.

Usage:

    rotate-consumer-bearer.py <consumer> <bearer-file> <holders> [action:item#field ...]
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
MIN_BEARER_CHARS = 32
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
    return subprocess.run(args, capture_output=True, text=True, check=False, env=ENVIRONMENT)


def encode(capability):
    field = capability.get("field")
    item = capability["item"]
    return f"{capability['action']}:{item}#{field}" if field else f"{capability['action']}:{item}"


def decode(text):
    action, _, rest = text.partition(":")
    item, _, field = rest.partition("#")
    if not action or not item:
        raise SystemExit(f"capability {text!r} is not action:item[#field]")
    return {"action": action, "item": item, "field": field} if field else {"action": action, "item": item}


def main():
    if len(sys.argv) < len(["self", "consumer", "bearer", "holders"]):
        raise SystemExit(
            "usage: rotate-consumer-bearer.py <consumer> <bearer-file> <holders> [action:item#field ...]"
        )
    consumer = sys.argv[ONE]
    bearer_file = pathlib.Path(sys.argv[ONE + ONE])
    holders = sys.argv[ONE + ONE + ONE]
    requested = [decode(text) for text in sys.argv[ONE + ONE + ONE + ONE :]]

    if not holders.isdigit() or int(holders) < ONE:
        raise SystemExit(
            f"holders={holders!r}: state how many hosts hold this bearer, measured from their "
            "configuration, and deliver the new file to every one of them"
        )
    if not bearer_file.is_file():
        raise SystemExit(f"no bearer file at {bearer_file}")
    bearer = bearer_file.read_text(encoding="utf-8").strip()
    if len(bearer) < MIN_BEARER_CHARS:
        raise SystemExit(f"{bearer_file} holds {len(bearer)} characters; that is not a bearer")

    document = json.loads(VAULT.read_text(encoding="utf-8"))
    grant = document.get("tokens", {}).get(consumer)
    if grant is NONE:
        raise SystemExit(f"no grant for consumer {consumer}; mint it deliberately first")
    existing = grant["capabilities"]
    remaining = grant["expires_at"] - int(time.time())
    supplied_hash = hashlib.sha256(bearer.encode()).hexdigest()
    print(f"consumer     {consumer}")
    print(f"capabilities {len(existing)} held")
    print(f"expires_in   {remaining} seconds")
    print(f"requested    {', '.join(encode(capability) for capability in requested) or '(none)'}")
    if remaining <= ZERO:
        raise SystemExit(f"the {consumer} grant expired; re-mint it deliberately instead")
    if supplied_hash == grant.get("hash"):
        raise SystemExit(
            f"{bearer_file} already hashes to the recorded bearer; use grant-consumer-field-read.py, "
            "which adds a capability without rotating anything"
        )

    union = list(existing)
    added = []
    for capability in requested:
        if capability in union:
            continue
        union.append(capability)
        added.append(encode(capability))
    print(f"adding       {', '.join(added) or '(nothing new)'}")

    backup = VAULT.with_suffix(f".before-{consumer}-bearer-rotation.json")
    shutil.copy2(VAULT, backup)
    print(f"backup       {backup}")

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
    if settled.get("hash") != supplied_hash:
        # The installed binary accepted `--token-file` and stored a bearer of its
        # own: this fleet's mac mini runs a `skarbiec 0.2.1` source build whose
        # `token-mint` predates that flag, and an unknown flag is not an error
        # there. The bearer it issued is in the mint's stdout and nowhere else, so
        # it is written to the file the holders read rather than discarded --
        # discarding it is what leaves a grant on a token no host has, which is
        # exactly the state this run had to repair.
        issued = NONE
        try:
            issued = json.loads(minted.stdout).get("token")
        except ValueError:
            issued = NONE
        if not isinstance(issued, str) or len(issued) < MIN_BEARER_CHARS:
            raise SystemExit(
                "the vault did not record the supplied bearer and the mint returned none; "
                f"the grant is on an unknown token and {backup} is the copy to restore"
            )
        bearer_file.write_text(issued + "\n", encoding="utf-8")
        bearer_file.chmod(0o600)
        supplied_hash = hashlib.sha256(issued.encode()).hexdigest()
        if settled.get("hash") != supplied_hash:
            raise SystemExit(
                "the vault recorded neither the supplied nor the returned bearer; "
                f"{backup} is the copy to restore"
            )
        print(f"bearer       the mint issued its own; persisted to {bearer_file.name}")
    print(f"capabilities {len(settled['capabilities'])} held (was {len(existing)})")
    print(f"bearer       rotated onto {bearer_file.name}; deliver it to the {holders} holder(s) now")
    print(f"expires_in   {settled['expires_at'] - int(time.time())} seconds")
    return NONE


sys.exit(main())
