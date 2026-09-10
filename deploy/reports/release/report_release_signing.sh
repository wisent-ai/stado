#!/usr/bin/env bash
# Report which release signing keys this host's Skarbiec actually holds, and who
# is authorized to read them.
#
# The 403 was never about a missing key. `stado-release-signing` exists with
# `kind=key-pair` and field `private_key`, and the vault authorizes exactly one
# consumer to read it: `stado-release-coordinator`, holding the single capability
# `read:stado-release-signing#private_key`. `release submit` asked as
# `stado-control-plane`, the broad grant, so the vault refused -- correctly.
#
# Two earlier versions of this probe reported the opposite, and the second one's
# conclusion reached a commit message and an architecture note before it was
# checked: that no release signer existed in any reachable vault. It came from a
# list truncated at ten rows, where `stado-release-signing` sorts eleventh. The
# same failure as calling a toolchain missing from a PATH-less shell, and as
# reading a store's `capacity/` prefix on a host whose queue store is a private
# loopback resolver while the fleet publishes to a tailnet address.
#
# So this reports authorization next to existence: an item nobody may read and an
# item that is absent are different failures with identical symptoms.
#
# Read-only: item ids, field names, and consumer names, never a secret value.
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

# Who may read each signing item. Existence without a reader looks exactly like
# absence from the caller's side, and that pair of symptoms is what sent two
# earlier readings of this vault the wrong way.
printf -- '--- authorized readers ---\n'
SKARBIEC_VAULT_FILE="$vault" "$cli" tokens 2>&1 | /usr/bin/python3 -c '
import json, sys

raw = sys.stdin.read()
try:
    tokens = json.loads(raw)
except json.JSONDecodeError:
    print(f"tokens unparsed: {raw.strip()[:120]}")
    raise SystemExit

readers = 0
for token in tokens if isinstance(tokens, list) else []:
    grants = [
        capability
        for capability in token.get("capabilities") or []
        if "sign" in str(capability.get("item") or "")
    ]
    if not grants:
        continue
    readers += 1
    parts = []
    for grant in grants:
        action = grant.get("action")
        item = grant.get("item")
        field = grant.get("field")
        parts.append(f"{action}:{item}#{field}")
    consumer = token.get("consumer")
    detail = ", ".join(parts)
    print(f"reader {consumer} -> {detail}")
print(f"signing_readers {readers}")
'
