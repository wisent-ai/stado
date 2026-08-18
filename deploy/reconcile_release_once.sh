#!/usr/bin/env bash
# Run one release reconciliation on this host and print the resulting phase.
#
# The resident reconciler was an unsupervised leftover and is stopped, so the state
# machine only advances when something asks it to. A blue-green rollout parks in
# `monitoring` for the strategy's rollback window and needs a later pass to record
# the release as active, which is exactly one `--once` run.
#
# Idempotent by design: reconciliation is the operation, and running it twice on a
# settled state changes nothing.
set -euo pipefail

stado="${STADO_BIN:-$HOME/.stado/bin/stado}"
# `hostname -s` answers `Control-host` while the registry target is
# `control-host`, and the agent compares them exactly: one more pair of names
# for one machine, this one mine.
target="${RELEASE_TARGET:-$(hostname -s 2>/dev/null | /usr/bin/tr '[:upper:]' '[:lower:]')}"
product="${RELEASE_PRODUCT:-brama}"
state_dir="${RELEASE_STATE_DIR:-$HOME/.stado/release-state}"

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'target %s\n' "$target"
[ -x "$stado" ] || { printf 'stado binary absent at %s\n' "$stado" >&2; exit 66; }

set +e
"$stado" release agent --target "$target" --once 2>&1 | /usr/bin/cut -c1-160
status=$?
set -e
printf 'agent_exit %s\n' "$status"

state="$state_dir/$product.json"
if [ -f "$state" ]; then
  /usr/bin/env python3 - "$state" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    state = json.load(handle)
for key in ("phase", "active_version", "previous_version", "rollout_generation", "detail"):
    print(f"  {key} {state.get(key)}")
PY
else
  printf '  %s absent\n' "$state"
fi

code=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' -m 5 "http://127.0.0.1:8080/health" 2>/dev/null || echo 000)
printf 'stable_bind_http %s\n' "$code"
