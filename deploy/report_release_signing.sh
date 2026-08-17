#!/usr/bin/env bash
# Report which release signing keys this host's vault actually holds.
#
# The registry's `release_control.trusted_keys` names a public key per product,
# so a keypair was generated once. `release submit` from an operator machine
# then failed with `Skarbiec returned HTTP 403: consumer not authorized to read
# item field` while reading the signing key's `private_key`, and that machine's
# vault holds no signing item at all. Whether the private half lives here decides
# between publishing from this host and generating new trust material.
#
# Read-only: item ids and field names only, never a secret value.
set -euo pipefail

vault="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"
printf 'host %s\n' "$(/bin/hostname -s)"
printf 'vault %s\n' "$vault"
if [ ! -r "$vault" ]; then
  printf 'vault unreadable\n'
  exit 0
fi

/usr/bin/python3 - "$vault" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    vault = json.load(handle)

items = vault.get("items")
# The store is a mapping of id -> item; older copies used a list of items.
entries = items.items() if isinstance(items, dict) else (
    (entry.get("id"), entry) for entry in (items or [])
)

matched = 0
for identifier, entry in entries:
    name = str(identifier or "")
    if "sign" not in name and "release" not in name:
        continue
    matched += 1
    value = entry.get("value") if isinstance(entry, dict) else None
    fields = sorted(value.keys()) if isinstance(value, dict) else []
    kind = (entry.get("kind") if isinstance(entry, dict) else None) or "?"
    print(f"item {name} kind={kind} fields={','.join(fields) or 'none'}")

print(f"matched_items {matched}")
PY
