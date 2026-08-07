#!/usr/bin/env python3
"""Re-issue one gateway verifier's bearer without changing what it may read.

A verifier fails closed with "consumer grant required" when the token file it
presents is older than the grant recorded in the vault. Minting rotates the
bearer, so the fix is to mint again with the identical capability set and write
the new value to the token file the config points at. The token is never
printed; only its consumer, capability count and destination are.

Usage: reissue-verifier-token.py <consumer>
"""
from __future__ import annotations

import json
import os
import pathlib
import stat
import subprocess
import sys

CONFIG = pathlib.Path(
    os.environ.get("STADO_CONFIG", str(pathlib.Path.home() / ".config/stado/config.json"))
)
SKARBIEC = pathlib.Path.home() / ".stado/bin/skarbiec"
SECTIONS = ("object_api", "release_api", "machine_api", "service_api", "integration")
OWNER_ONLY = stat.S_IRUSR | stat.S_IWUSR


def expand(raw: str) -> pathlib.Path:
    home = str(pathlib.Path.home())
    return pathlib.Path(raw.replace("~", home) if raw.startswith("~") else raw)


def token_file_for(consumer: str) -> pathlib.Path:
    config = json.loads(CONFIG.read_text())
    for section in SECTIONS:
        skarbiec = config.get(section, {}).get("skarbiec", {})
        if skarbiec.get("consumer") == consumer:
            return expand(skarbiec["token_file"])
    # Integration provider domains carry their own grant per domain rather
    # than one verifier per section.
    for domain, policy in config.get("integration", {}).get("providers", {}).items():
        if policy.get("consumer") == consumer:
            print(f"consumer belongs to integration provider domain {domain}")
            return expand(policy["token_file"])
    raise SystemExit(f"{consumer} is declared by no section of {CONFIG}")


def run(args: list[str]) -> str:
    result = subprocess.run(
        [str(SKARBIEC), *args],
        capture_output=True,
        text=True,
        env={**os.environ, "SKARBIEC_VAULT_FILE": os.environ.get(
            "SKARBIEC_VAULT_FILE", str(pathlib.Path.home() / ".stado/skarbiec.vault.json")
        )},
        check=False,
    )
    if result.returncode:
        raise SystemExit(f"skarbiec {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def main() -> int:
    if len(sys.argv) < len("xx"):
        print(__doc__)
        return len("x")
    consumer = sys.argv[len("x")]
    destination = token_file_for(consumer)
    grants = json.loads(run(["tokens"]))
    current = next((row for row in grants if row.get("consumer") == consumer), None)
    if current is None:
        raise SystemExit(f"the vault records no grant for {consumer}")
    capabilities = [
        f"{cap['action']}:{cap['item']}" + (f"#{cap['field']}" if cap.get("field") else "")
        for cap in current.get("capabilities", [])
    ]
    if not capabilities:
        raise SystemExit(f"{consumer} has an empty capability set; refusing to mint a blank grant")
    minted = json.loads(
        run(["token-mint", consumer, "--capabilities", ",".join(capabilities), "--replace-capabilities"])
    )
    destination.write_text(minted["token"])
    destination.chmod(OWNER_ONLY)
    print(
        json.dumps(
            {
                "consumer": consumer,
                "capabilities": len(capabilities),
                "token_file": str(destination),
                "audience": minted.get("audience"),
            },
            sort_keys=True,
        )
    )
    return len("")


if __name__ == "__main__":
    sys.exit(main())
