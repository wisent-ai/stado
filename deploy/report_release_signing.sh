#!/usr/bin/env bash
# Report which release signing keys this host's Skarbiec actually holds.
#
# The registry's `release_control.trusted_keys` names a public key per product,
# so a keypair was generated once. `release submit` from an operator machine then
# failed with `Skarbiec returned HTTP 403: consumer not authorized to read item
# field` while reading the signing key's `private_key`.
#
# That 403 came from the Skarbiec service, so the service is what must be asked.
# An earlier version of this probe parsed the on-disk vault file instead and was
# about to support the much stronger claim that no signer exists anywhere -- the
# same mistake as reading a PATH-less shell and calling a toolchain missing.
#
# Read-only: item ids and field names only, never a secret value.
set -euo pipefail

cli="${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}"
printf 'host %s\n' "$(/bin/hostname -s)"
printf 'cli %s\n' "$cli"
if [ ! -x "$cli" ]; then
  printf 'cli unavailable\n'
  exit 0
fi

# The CLI's own default vault path and the path the services run against differ
# on at least one host -- the default answered "vault not initialized" while the
# services' file held seventeen items. Ask about the file the services use, and
# print which one that was, so the divergence is part of the answer rather than
# the reason the probe failed.
vault="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"
printf 'asked_vault %s\n' "$vault"
printf -- '--- service store ---\n'
SKARBIEC_VAULT_FILE="$vault" "$cli" list 2>&1 | /usr/bin/python3 -c '
import json, sys

raw = sys.stdin.read()
try:
    items = json.loads(raw)
except json.JSONDecodeError:
    print(f"list unparsed: {raw.strip()[:120]}")
    raise SystemExit

matched = 0
for entry in items if isinstance(items, list) else []:
    name = str(entry.get("id") or "")
    if "sign" not in name and "release" not in name:
        continue
    matched += 1
    kind = entry.get("kind") or "?"
    print(f"item {name} kind={kind}")
print(f"matched_items {matched}")
'

printf -- '--- vault file ---\n'
if [ -r "$vault" ]; then
  /usr/bin/python3 - "$vault" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    vault = json.load(handle)
items = vault.get("items")
names = list(items.keys()) if isinstance(items, dict) else [
    str((entry or {}).get("id") or "") for entry in (items or [])
]
signing = [name for name in names if "sign" in name]
print(f"file_items {len(names)} signing_named {len(signing)}")
for name in sorted(signing):
    print(f"file_item {name}")
PY
else
  printf 'vault file unreadable %s\n' "$vault"
fi
