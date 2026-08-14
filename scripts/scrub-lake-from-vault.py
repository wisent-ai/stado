#!/usr/bin/env python3
"""Standing repair: keep every value Skarbiec holds out of the transcript archive.

Masking catches secret-bearing SHAPES at write time. It cannot catch a human typing
a password into a chat as prose, and it cannot catch a transcript imported years
later. The vault is the one place that knows what the fleet's secrets actually are,
so this job asks it, and rewrites any value it finds in the Lake to the masker's own
`[masked:credential:…]` marker.

How it stays safe:

- Values are read into memory and never printed, written to a file, or passed on a
  command line: the literal list reaches the scrub on standard input, and only
  lengths, digests and counts are logged.
- A literal is only material if it can plausibly be a secret in text: at least
  twelve characters, at least three of the four character classes, no newline (a
  PEM cannot match the escaped form stored in a partition anyway), and never the
  `username` field. Everything rejected is counted, never named.
- The scrub itself counts before it writes and refuses any literal that occurs in
  more files than its cap, so one wrong value in the vault costs a report rather
  than the archive.
- Idempotent: a run that finds nothing rewrites nothing and reports zero.

Residual, stated plainly because it is the point: a secret that was never put in
the vault cannot be scrubbed by this job. The archive is exactly as clean as the
vault is complete, which is the argument for putting credentials in Skarbiec rather
than in a chat message.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time

NONE = None
ZERO = len("")
HOME = pathlib.Path(os.environ.get("HOME") or os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
SCRUB = (
    HOME
    / "Documents"
    / "CodingProjects"
    / "Wisent"
    / "transcript-lake"
    / "scripts"
    / "scrub-known-secret.py"
)
PYTHON = "/usr/bin/python3"
LAKE = pathlib.Path(os.environ.get("LAKE_DATA") or (HOME / ".transcript-lake"))
# Both vaults this host owns, named explicitly: ~/.stado holds around twenty
# *vault*.json files, most of them pre-migration snapshots, and scrubbing against a
# stale one would both miss current secrets and resurrect retired ones.
VAULTS = (
    HOME / ".stado" / "skarbiec.vault.json",
    HOME / ".stado" / "weles-skarbiec.vault.json",
)
# Fields that are identifiers rather than secrets. Everything else an item carries
# is treated as material, because the fleet's secrets live in free-form kinds
# (bundle, stado-secret, internal-authority) whose field names nobody maintains a
# list of.
IDENTIFIER_FIELDS = ("username", "user", "login", "email", "account", "id", "url", "host")
MIN_LENGTH = len("a" * 12)
MIN_CLASSES = len("abc")
TIMEOUT = 3600


def digest(value):
    return hashlib.sha256(value.encode("utf-8")).hexdigest()[: len("a" * 8)]


def classes(value):
    """How many of lower, upper, digit, symbol the value draws on."""
    return sum(
        (
            any(character.islower() for character in value),
            any(character.isupper() for character in value),
            any(character.isdigit() for character in value),
            any(not character.isalnum() for character in value),
        )
    )


def material(value):
    """Whether a stored value could occur as a secret inside transcript text."""
    return (
        isinstance(value, str)
        and len(value) >= MIN_LENGTH
        and "\n" not in value
        and "\r" not in value
        and classes(value) >= MIN_CLASSES
    )


def run(argv, vault, stdin=NONE):
    return subprocess.run(
        argv,
        input=stdin,
        capture_output=True,
        text=True,
        check=False,
        timeout=TIMEOUT,
        env={
            **os.environ,
            "HOME": str(HOME),
            "SKARBIEC_VAULT_FILE": str(vault),
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
        },
    )


def collect(vault):
    """Every value in one vault that could be a secret in text, plus counters.

    `list` returns envelope metadata only, so it is safe to log; `get` returns
    decrypted fields, so its output is consumed here and never surfaces."""
    if not vault.is_file():
        print(f"vault {vault} absent")
        return set(), {}
    listing = run([str(SKARBIEC), "list"], vault)
    if listing.returncode != ZERO:
        print(f"vault {vault} unreadable: {listing.stderr.strip().splitlines()[-1:]}")
        return set(), {"unreadable": 1}
    items = json.loads(listing.stdout)
    values = set()
    tally = {"items": len(items), "read_failed": ZERO, "fields": ZERO, "rejected": ZERO}
    for item in items:
        if item.get("deleted"):
            continue
        read = run([str(SKARBIEC), "get", item["id"]], vault)
        if read.returncode != ZERO:
            tally["read_failed"] += 1
            continue
        try:
            document = json.loads(read.stdout)
        except ValueError:
            tally["read_failed"] += 1
            continue
        fields = document.get("fields")
        if not isinstance(fields, dict):
            continue
        for name, value in fields.items():
            if name.lower() in IDENTIFIER_FIELDS:
                continue
            tally["fields"] += 1
            if material(value):
                values.add(value)
            else:
                tally["rejected"] += 1
    print(
        f"vault {vault.name} items={tally['items']} fields={tally['fields']}"
        f" material={len(values)} rejected_as_not_secret={tally['rejected']}"
        f" unreadable_items={tally['read_failed']}"
    )
    return values, tally


def main():
    started = time.time()
    print(f"started {time.strftime('%Y-%m-%dT%H:%M:%S%z')}")
    if not SCRUB.is_file():
        raise SystemExit(f"no scrub script at {SCRUB}")
    if not SKARBIEC.is_file():
        raise SystemExit(f"no skarbiec binary at {SKARBIEC}")
    if not LAKE.is_dir():
        raise SystemExit(f"no Lake at {LAKE}")

    literals = set()
    for vault in VAULTS:
        values, _ = collect(vault)
        literals |= values
    print(f"literals from the vaults {len(literals)}")
    for value in sorted(literals, key=digest):
        print(f"  literal len={len(value)} sha256[:8]={digest(value)}")
    if not literals:
        print("nothing to scrub; the vaults hold no value that could occur in text")
        print(f"finished in {time.time() - started:.1f}s")
        return ZERO

    scrub = run(
        [
            PYTHON,
            str(SCRUB),
            "--secret-file",
            "-",
            "--data-dir",
            str(LAKE),
            "--apply",
        ],
        VAULTS[ZERO],
        stdin="\n".join(sorted(literals)),
    )
    for line in scrub.stdout.splitlines():
        print("  " + line[: len("a" * 200)])
    for line in scrub.stderr.splitlines():
        print("  stderr " + line[: len("a" * 200)])
    print(f"scrub exit {scrub.returncode}")
    print(f"finished in {time.time() - started:.1f}s")
    # A refused literal (over its file cap) is a report, not a failure of the run:
    # exit non-zero so the operator sees it, after the accepted ones were applied.
    return scrub.returncode


sys.exit(main())
