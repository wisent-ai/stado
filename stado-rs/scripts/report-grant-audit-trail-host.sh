#!/bin/sh
# Who last minted the worker-agent grant, and what else touches it.
#
# After a rotation the vault recorded a hash that matches neither the delivered
# bearer nor any obvious encoding of it, which means the last writer was not the
# rotation. Skarbiec keeps an audit entry per mint, and this fleet also installs
# reauth jobs that re-mint grants on a schedule, so the trail decides it.
#
# Read-only. Audit metadata only: consumer, action, timestamp, capability count.
set -eu

skarbiec="$HOME/.stado/bin/skarbiec"

printf 'AUDIT_TAIL\n'
if [ -x "$skarbiec" ]; then
  "$skarbiec" audit 2>/dev/null |
    /usr/bin/python3 -c '
import json, sys
try:
    entries = json.load(sys.stdin)
except ValueError:
    print("audit output is not json")
    raise SystemExit(0)
rows = entries.get("entries") if isinstance(entries, dict) else entries
rows = rows or []
interesting = [
    row for row in rows
    if "token" in str(row.get("action", "")) or "stado-local-agent" in json.dumps(row)
]
for row in interesting[-8:]:
    payload = row.get("payload") or {}
    capabilities = payload.get("capabilities")
    print(
        row.get("at") or row.get("timestamp") or "?",
        row.get("action"),
        payload.get("consumer") or "",
        f"caps={len(capabilities) if isinstance(capabilities, list) else '"'"'?'"'"'}",
    )
' 2>/dev/null || printf 'audit unavailable\n'
else
  printf 'no skarbiec binary\n'
fi

printf '\nREAUTH_UNITS\n'
for root in "$HOME/Library/LaunchAgents" /Library/LaunchDaemons; do
  [ -d "$root" ] || continue
  grep -rl -i 'reauth\|token-mint' "$root" 2>/dev/null || true
done | sort -u | head -10
printf '\n'

printf 'REAUTH_CONFIG\n'
ls -1 "$HOME"/.stado/*reauth* 2>/dev/null | head -10 || printf 'none\n'
