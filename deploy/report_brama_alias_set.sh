#!/usr/bin/env bash
# Report the model alias set this host's brama policy allows.
#
# The release agent quarantined brama's candidate with "candidate did not become
# ready before deadline". The candidate's launcher exits on one line --
# `services.brama.allowed_models must contain the exact closed Brama alias set` --
# because it compares the policy against a set hard-coded in
# `scripts/start-with-skarbiec.sh`. Two declarations of one alias set, and the
# comparison is the only thing that notices they disagree.
#
# This prints both sides' contents so the difference is a fact rather than an
# inference. Model aliases are not secrets; no credential is read or printed.
set -euo pipefail

# The launcher validates `$BRAMA_CONTROL_CONFIG`, defaulting to
# `~/.config/brama/control.json`, and takes that variable from
# `~/.config/brama/service.env`. An earlier version of this probe read
# `~/.config/brama/trust/policy.json` instead and reported the key absent from a
# file nothing checks -- the same wrong-surface answer that has cost this session
# five confident mistakes already.
policy="${BRAMA_CONTROL_CONFIG:-}"
service_env="${BRAMA_SERVICE_ENV:-$HOME/.config/brama/service.env}"
if [ -z "$policy" ] && [ -f "$service_env" ]; then
  policy=$(/usr/bin/sed -n 's/^[[:space:]]*BRAMA_CONTROL_CONFIG[[:space:]]*=[[:space:]]*//p' "$service_env" \
    | /usr/bin/tail -1 | /usr/bin/tr -d "\"'")
fi
policy="${policy:-$HOME/.config/brama/control.json}"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'service_env %s\n' "$service_env"
printf 'control_config %s\n' "$policy"
if [ ! -f "$policy" ]; then
  printf 'control_config absent\n'
  exit 0
fi

/usr/bin/env python3 - "$policy" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    policy = json.load(handle)

services = policy.get("services") or {}
brama = services.get("brama") or {}
allowed = brama.get("allowed_models")
if allowed is None:
    print("allowed_models absent from services.brama")
    raise SystemExit

print(f"allowed_models count={len(allowed) if isinstance(allowed, list) else 'not-a-list'}")
for alias in allowed if isinstance(allowed, list) else []:
    print(f"  {alias}")

# The launcher's closed set, copied from scripts/start-with-skarbiec.sh at the
# revision this host is being asked to run.
expected = [
    "best",
    "wisent-backend/chat/primary",
    "wisent-backend/chat/fallback",
    "wisent-backend/evaluation",
    "wisent-backend/embeddings",
    "wisent-backend/moderation",
    "weles/agent/primary",
]
have = set(allowed) if isinstance(allowed, list) else set()
want = set(expected)
print(f"launcher_expects count={len(want)}")
print(f"missing_from_policy {sorted(want - have) or 'none'}")
print(f"extra_in_policy {sorted(have - want) or 'none'}")
PY

# What a candidate reads. The release agent passes only `runtime.environment` from
# the product manifest, and brama's manifest declares none, so the launcher falls
# back to `~/.config/brama/control.json` while the running service is configured
# through `service.env` to a different file entirely. That is why the candidate
# fails the alias-set check that the stable process passes.
fallback="$HOME/.config/brama/control.json"
printf -- '--- launcher fallback ---\n'
printf 'fallback_path %s\n' "$fallback"
if [ "$fallback" = "$policy" ]; then
  printf 'fallback is the configured file; candidate and service agree\n'
elif [ ! -f "$fallback" ]; then
  printf 'fallback absent: a candidate launched without BRAMA_CONTROL_CONFIG has no policy\n'
else
  /usr/bin/env python3 - "$fallback" <<'PY'
import json, sys

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        document = json.load(handle)
except (OSError, json.JSONDecodeError) as error:
    print(f"fallback unreadable: {error}")
    raise SystemExit
brama = (document.get("services") or {}).get("brama") or {}
allowed = brama.get("allowed_models")
if allowed is None:
    print("fallback has no services.brama.allowed_models")
else:
    print(f"fallback allowed_models count={len(allowed)}")
PY
fi
