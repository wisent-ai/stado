#!/usr/bin/env python3
"""Mint a gateway verifier's grant from the items its config section declares.

The validator requires the verifier to see exactly the mapped item set. A grant
drifts when a mapping is retired: the capability stays, the item goes to trash,
and every later mint fails with "item is in trash" while the server answers
"consumer grant required". Reading the declared set from the config and minting
that, instead of re-minting whatever the grant happens to hold, keeps the two
definitions from diverging again.

Usage: align-verifier-grant.py <object_api|release_api|machine_api|service_api>
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
MAPPING_KEY = {
    "object_api": "namespaces",
    "release_api": "publishers",
    "machine_api": "clients",
    "service_api": "deployers",
}
OWNER_ONLY = stat.S_IRUSR | stat.S_IWUSR


def expand(raw: str) -> pathlib.Path:
    home = str(pathlib.Path.home())
    return pathlib.Path(raw.replace("~", home) if raw.startswith("~") else raw)


def run(args: list[str]) -> str:
    result = subprocess.run(
        [str(SKARBIEC), *args],
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "SKARBIEC_VAULT_FILE": os.environ.get(
                "SKARBIEC_VAULT_FILE", str(pathlib.Path.home() / ".stado/skarbiec.vault.json")
            ),
        },
        check=False,
    )
    if result.returncode:
        raise SystemExit(f"skarbiec {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def main() -> int:
    if len(sys.argv) < len("xx"):
        print(__doc__)
        return len("x")
    section = sys.argv[len("x")]
    if section not in MAPPING_KEY:
        raise SystemExit(f"unknown section {section!r}; expected one of {', '.join(MAPPING_KEY)}")
    config = json.loads(CONFIG.read_text())
    block = config.get(section, {})
    skarbiec = block.get("skarbiec", {})
    consumer = skarbiec.get("consumer")
    destination = expand(skarbiec.get("token_file", ""))
    if not consumer or not str(destination):
        raise SystemExit(f"{section} declares no verifier consumer and token_file")
    declared = sorted(
        {policy["item"] for policy in block.get(MAPPING_KEY[section], {}).values()}
    )
    live = {row["id"] for row in json.loads(run(["list"]))}
    missing = [item for item in declared if item not in live]
    if missing:
        raise SystemExit(
            f"{section} declares items that do not exist in the vault: {', '.join(missing)}"
        )
    capabilities = [f"read:{item}#token" for item in declared]
    minted = json.loads(
        run(["token-mint", consumer, "--capabilities", ",".join(capabilities), "--replace-capabilities"])
    )
    destination.write_text(minted["token"])
    destination.chmod(OWNER_ONLY)
    print(
        json.dumps(
            {"section": section, "consumer": consumer, "items": declared, "token_file": str(destination)},
            sort_keys=True,
        )
    )
    return len("")


if __name__ == "__main__":
    sys.exit(main())
