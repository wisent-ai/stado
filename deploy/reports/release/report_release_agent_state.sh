#!/usr/bin/env bash
# Report why this host's release agent quarantined a desired release.
#
# `release status` says `observed=quarantined` with the detail "desired release
# digest is quarantined on this host", which names neither the digest's reason nor
# when it was recorded. The agent writes both into its state file, and the reason
# separates a fetch or signature failure from a candidate that started and never
# became ready -- opposite problems with one symptom.
#
# Read-only: phases, versions, reasons and timestamps. No archive is fetched and
# no service is touched.
set -euo pipefail

state_dir="${RELEASE_STATE_DIR:-$HOME/.stado/release-state}"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'state_dir %s\n' "$state_dir"
if [ ! -d "$state_dir" ]; then
  printf 'state_dir absent\n'
  exit 0
fi

for state in "$state_dir"/*.json; do
  [ -f "$state" ] || continue
  printf -- '--- %s ---\n' "$(basename "$state")"
  /usr/bin/env python3 - "$state" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    state = json.load(handle)

print(f"  phase {state.get('phase')}")
print(f"  active_version {state.get('active_version')}")
print(f"  previous_version {state.get('previous_version')}")
print(f"  rollout_generation {state.get('rollout_generation')}")
print(f"  detail {state.get('detail')}")
quarantined = state.get("quarantined") or {}
print(f"  quarantined_digests {len(quarantined)}")
for digest, record in quarantined.items():
    reason = (record or {}).get("reason") if isinstance(record, dict) else record
    when = (record or {}).get("quarantined_at") if isinstance(record, dict) else ""
    print(f"    {digest[:12]} at={when}")
    print(f"      reason: {reason}")
PY
done
