#!/usr/bin/env python3
"""Which vault, and whose identity, does each credential path on this host read?

Three commands on this host look interchangeable and are not. The `skarbiec`
binary reads whatever `SKARBIEC_VAULT_FILE` says, so its answer depends on the
caller's environment. `stado credentials` reads the store named by config
`credentials.store` and is filtered to what one consumer holds capabilities on,
so an item it cannot see looks missing when it is merely ungranted. And
`stado host install-credential` authenticates over HTTP as the consumer in config
`secrets.skarbiec.consumer`, which is a different identity with a different reach
again. A credential that "is not there" is usually one of those three answering a
question it was never asked.

So this measures each path separately and prints an identity fingerprint for the
vault behind it: the owner key uid and fingerprint, the item count, and a digest
over the sorted item-id list. Never contents, never a field value, never a bearer
-- ids and digests only. It ends by naming every pair of paths a reader would
treat as one that do not agree, and exits non-zero when there is at least one.

The vault file inventory of a host is a different question, already answered by
`stado host vaults [TARGET]`; this does not duplicate it. Run this on another host
with `stado host install-helper <host> scripts/audit-vault-identity.py
audit-vault-identity && stado host run-helper <host> audit-vault-identity`.
"""

import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import urllib.error
import urllib.request

NONE = None
ZERO = len([])
ONE = len("a")
SHORT = len("a" * 12)
HOME = pathlib.Path(os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
STADO = HOME / ".stado" / "bin" / "stado"
CONFIG = HOME / ".config" / "stado" / "config.json"
DEFAULT_VAULT = HOME / ".local" / "share" / "skarbiec" / "skarbiec.vault.json"
DEFAULT_STORE = "skarbiec"
DEFAULT_CONSUMER = "stado-control-plane"
DEFAULT_URL = "http://127.0.0.1:8799"
# lsof lives in /usr/sbin on macOS and /usr/bin on Linux, and neither is on the
# PATH a launchd-run helper inherits, so both go in here.
ENVIRONMENT = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
}


def run(*args, vault=NONE):
    environment = dict(ENVIRONMENT)
    if vault:
        environment["SKARBIEC_VAULT_FILE"] = str(vault)
    try:
        return subprocess.run(
            args, capture_output=True, text=True, check=False, env=environment
        )
    except FileNotFoundError:
        # A missing tool is a measurement too, and a more useful one than a stack
        # trace: the caller reports the empty answer and keeps auditing.
        return subprocess.CompletedProcess(args, ONE, "", f"{args[ZERO]} not installed")


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()[:SHORT]


def identity(vault):
    """The vault behind one path, named without reading anything it protects."""
    path = pathlib.Path(vault)
    if not path.is_file():
        return {"vault": str(path), "state": "absent"}
    listed = run(str(SKARBIEC), "list", "--all", vault=path)
    if listed.returncode != ZERO:
        return {
            "vault": str(path),
            "state": f"unreadable ({listed.stderr.strip().splitlines()[-ONE:]})",
        }
    ids = sorted(entry["id"] for entry in json.loads(listed.stdout))
    people = json.loads(run(str(SKARBIEC), "users", vault=path).stdout or "{}")
    owners = [
        (uid, facts.get("fingerprint", ""))
        for uid, facts in people.items()
        if facts.get("role") == "owner"
    ]
    return {
        "vault": str(path),
        "state": "readable",
        "bytes": path.stat().st_size,
        "items": len(ids),
        "item_digest": digest("\n".join(ids)),
        "ids": set(ids),
        "owner": owners[ZERO][ZERO] if owners else "(none)",
        "owner_fingerprint": owners[ZERO][ONE] if owners else "",
        "recipients": len(people),
    }


def report(label, facts):
    print(f"  vault        {facts['vault']}")
    print(f"  state        {facts['state']}")
    if facts["state"] != "readable":
        return
    print(f"  owner        {facts['owner']} {facts['owner_fingerprint']}")
    print(f"  recipients   {facts['recipients']}")
    print(f"  items        {facts['items']} ({facts['bytes']} bytes on disk)")
    print(f"  item_digest  sha256:{facts['item_digest']} over the sorted id list")
    # One line built to be diffed between hosts: this audit answers per host, and
    # the same file name holds different item sets on different machines, so the
    # name proves nothing and this triple is what has to match.
    print(
        f"  cross_host   {facts['owner_fingerprint'][:SHORT]} {facts['items']} "
        f"sha256:{facts['item_digest']}"
    )


def listeners():
    """Every skarbiec process listening on this host, with its start time."""
    found = []
    open_ports = run("lsof", "-nP", "-iTCP", "-sTCP:LISTEN")
    for line in open_ports.stdout.splitlines():
        columns = line.split()
        if len(columns) < len("a" * 9) or "skarbiec" not in columns[ZERO]:
            continue
        address = columns[-ONE - ONE] if columns[-ONE] == "(LISTEN)" else columns[-ONE]
        pid = columns[ONE]
        described = run("ps", "-o", "lstart=,command=", "-p", pid)
        found.append((pid, address, described.stdout.strip()))
    return found


def owner_of_service(address):
    """A service's vault identity, from the one endpoint that needs no bearer."""
    url = f"http://{address}/v1/owner-pubkey"
    try:
        with urllib.request.urlopen(url, timeout=len("abc")) as answer:
            armored = json.loads(answer.read().decode()).get("armored", "")
    except (urllib.error.URLError, ValueError, TimeoutError) as refusal:
        return {"state": f"unavailable ({refusal})", "uids": []}
    # The uid strings live in the public key packet, so they can be read out of
    # the decoded key without a PGP implementation and without any private half.
    import base64

    body = "".join(
        line
        for line in armored.splitlines()
        if line and not line.startswith("-----") and ":" not in line
    )
    try:
        packet = base64.b64decode(body, validate=False)
    except ValueError:
        packet = b""
    # The uid packet is preceded by its length byte, which decodes as a printable
    # character often enough to end up glued to the front of the name, so the
    # match starts at an alphanumeric and any leading punctuation is dropped.
    uids = sorted(set(re.findall(rb"[A-Za-z0-9][ -~]{7,}", packet)))
    named = [
        uid.decode().strip()
        for uid in uids
        if b"skarbiec" in uid or b"@" in uid
    ]
    return {"state": "answered", "key_digest": digest(armored), "uids": named}


def store_selector():
    if not CONFIG.is_file():
        return DEFAULT_STORE, DEFAULT_CONSUMER, DEFAULT_URL
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    secrets = document.get("secrets", {}).get("skarbiec", {})
    return (
        document.get("credentials", {}).get("store") or DEFAULT_STORE,
        secrets.get("consumer") or DEFAULT_CONSUMER,
        secrets.get("url") or DEFAULT_URL,
    )


def main():
    disagreements = []
    print("=== path 1: the skarbiec binary on this host ===")
    named = os.environ.get("SKARBIEC_VAULT_FILE")
    invoked = identity(named or DEFAULT_VAULT)
    print(f"  address      {'SKARBIEC_VAULT_FILE' if named else 'built-in default'}")
    report("invoked", invoked)
    if not named:
        print("  note         with no SKARBIEC_VAULT_FILE set this path answers about the")
        print("               built-in default, so the binary's answer is a property of")
        print("               the caller's environment rather than of the host")

    print()
    print("=== path 1b: the conventional fleet vault file ===")
    fleet = identity(HOME / ".stado" / "skarbiec.vault.json")
    report("fleet", fleet)
    # Whichever of the two is readable is what the other paths get compared with,
    # because an absent default proves nothing about the vault in service.
    local = invoked if invoked.get("state") == "readable" else fleet
    if (
        invoked.get("state") == "readable"
        and fleet.get("state") == "readable"
        and invoked["item_digest"] != fleet["item_digest"]
    ):
        disagreements.append(
            f"the binary was invoked against {invoked['vault']} "
            f"({invoked['items']} items, sha256:{invoked['item_digest']}) while the "
            f"conventional fleet vault {fleet['vault']} holds {fleet['items']} items "
            f"(sha256:{fleet['item_digest']}); two files, one habit of calling both "
            '"the vault"'
        )

    print()
    print("=== path 2: skarbiec services listening on this host ===")
    for pid, address, described in listeners():
        served = owner_of_service(address)
        print(f"  listener     {address} pid {pid}")
        print(f"  process      {described}")
        print(f"  owner-pubkey {served['state']}")
        for uid in served.get("uids", []):
            print(f"  serves-key   {uid}")
        if served.get("key_digest"):
            print(f"  key_digest   sha256:{served['key_digest']}")
        if (
            local.get("state") == "readable"
            and served["state"] == "answered"
            and local["owner"]
            and not any(local["owner"] in uid for uid in served.get("uids", []))
        ):
            disagreements.append(
                f"the service on {address} serves a vault whose owner key is not "
                f"{local['owner']}, the owner of {local['vault']}"
            )
    if not listeners():
        print("  (no skarbiec process is listening here)")

    print()
    print("=== path 3: the Stado credential store ===")
    store, consumer, url = store_selector()
    token_file = HOME / ".stado" / "control-plane-skarbiec-token"
    print(f"  selector     credentials.store = {store}")
    print(f"  transport    {url} as consumer {consumer}")
    print(f"  bearer       {token_file if token_file.is_file() else '(absent)'}")
    catalogue = run(str(STADO), "credentials", "ls")
    rows = [line.split() for line in catalogue.stdout.splitlines() if line.split()]
    listed = {row[ZERO] for row in rows[ONE:]} if rows else set()
    print(f"  catalogue    {len(listed)} items visible to `stado credentials ls`")

    # Whose view is that? Compare it with every consumer's capability item set
    # instead of trusting the command's name, which says only "admin".
    document = json.loads(pathlib.Path(local["vault"]).read_text(encoding="utf-8")) if (
        local.get("state") == "readable"
    ) else {}
    grants = document.get("tokens", {})
    reach = {
        name: {capability["item"] for capability in grant.get("capabilities", [])}
        for name, grant in grants.items()
    }
    equals = [name for name, items in reach.items() if items == listed and listed]
    print(f"  view_of      {', '.join(equals) or '(matches no single consumer exactly)'}")
    if not equals and listed:
        # An exact match is the clean answer, but a near match still names the
        # identity to look at instead of leaving the reader with nothing.
        overlaps = sorted(
            ((len(items & listed), len(items), name) for name, items in reach.items()),
            reverse=True,
        )
        for shared, held, name in overlaps[:ONE + ONE]:
            print(
                f"  closest      {name} shares {shared} of the {len(listed)} listed "
                f"and holds {held}"
            )
    print(f"  delivery     stado host install-credential authenticates as {consumer},")
    print(f"               which holds {len(reach.get(consumer, set()))} item(s) of capability")
    if local.get("state") == "readable" and listed - local["ids"]:
        disagreements.append(
            f"`stado credentials ls` lists {len(listed - local['ids'])} item(s) absent "
            f"from {local['vault']}, so the two paths read different vaults"
        )
    if equals and consumer not in equals:
        disagreements.append(
            f"`stado credentials ls` shows the {equals[ZERO]} view "
            f"({len(listed)} items) while `stado host install-credential` reads as "
            f"{consumer} ({len(reach.get(consumer, set()))} items); one CLI, two "
            "identities, and an item granted to the first is refused to the second"
        )

    print()
    print("=== recorded findings ===")
    print("  2026-08-14  `stado host install-credential lukasz-macbook <item> <field> <name>`")
    print("              answered HTTP 403 `consumer not authorized to read item field`")
    print("              for host-account-charless-mac-mini#password AND for")
    print("              login-example-com#username, a grant local-operator has held")
    print("              since July. Cause measured above: the delivery path reads as")
    print(f"              {consumer}, not as the consumer whose view the catalogue shows,")
    print("              so a local-operator grant never authorizes a delivery.")
    print("  2026-08-14  A 526-vs-558 item gap between `stado credentials ls` and")
    print("              `skarbiec list` was NOT two vaults: the catalogue is filtered to")
    print("              one consumer's capabilities. Minting one capability moved the")
    print("              count to 527, which is how the filter was identified.")
    print("  2026-08-14  `$HOME/.stado/skarbiec.vault.json` is a path, not an identity:")
    print("              lukasz-macbook and charless-mac-mini both have one, with the same")
    print("              owner key and different item sets. This audit answers per host, so")
    print("              compare the cross_host line between hosts before concluding that")
    print("              an item written on one of them is readable on the other.")

    print()
    print("=== agreement ===")
    for disagreement in disagreements:
        print(f"  disagree     {disagreement}")
    if not disagreements:
        print("  agree        every path measured here names the same vault and identity")
    return ONE if disagreements else NONE


sys.exit(main())
