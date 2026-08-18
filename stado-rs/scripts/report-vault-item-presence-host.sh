#!/bin/sh
# Does this vault hold a Vast.ai credential at all, under any of its names?
#
# `token-mint` refused `read:stado-vast#api_key` with "capability names a missing
# item", which is a different fact from the 403 the broker returns for a field a
# consumer may not read: the item itself is absent. Before concluding that, the
# neighbouring spellings are worth listing, because a grant that names the wrong
# id fails exactly the same way.
#
# Read-only, and item ids only: no value, no field, nothing decrypted.
set -eu

skarbiec="$HOME/.stado/bin/skarbiec"
[ -x "$skarbiec" ] || { printf 'ERROR\tno skarbiec at %s\n' "$skarbiec" >&2; exit 1; }

printf 'ITEMS_MATCHING_VAST\n'
"$skarbiec" list 2>/dev/null | grep -i vast || printf 'none\n'

printf '\nITEMS_MATCHING_MARKETPLACE\n'
"$skarbiec" list 2>/dev/null | grep -i -E 'marketplace|rental|renter' || printf 'none\n'

printf '\nGRANT_CAPABILITIES_FOR_WORKER_AGENT\n'
"$skarbiec" tokens 2>/dev/null |
  /usr/bin/python3 -c '
import json, sys
document = json.load(sys.stdin)
grants = document if isinstance(document, dict) else {}
grant = (grants.get("tokens") or grants).get("stado-local-agent")
if not grant:
    print("no grant recorded for stado-local-agent")
    raise SystemExit(0)
capabilities = grant.get("capabilities") or []
print(f"count {len(capabilities)}")
for capability in capabilities:
    field = capability.get("field")
    item = capability.get("item")
    print(f"  {capability.get(chr(97) + chr(99) + chr(116) + chr(105) + chr(111) + chr(110))}:{item}" + (f"#{field}" if field else ""))
' 2>/dev/null || printf 'tokens listing unavailable\n'
