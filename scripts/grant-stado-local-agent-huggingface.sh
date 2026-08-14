#!/bin/sh
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

skarbiec="$HOME/.stado/bin/skarbiec"
vault="$HOME/.stado/skarbiec.vault.json"
grant="$HOME/.stado/local-agent-skarbiec-token"
hf_source="${HF_TOKEN_FILE:-$HOME/.cache/huggingface/token}"
capabilities='read:compute-marketplace-agent#token,read:jeden-model-router#token,read:jeden-agent-auth#agent_auth_secret,read:stado-huggingface#token,read:trading-autonomy-agent-auth#token,read:trading-autonomy-media-router#token,read:trading-autonomy-model-router#token,read:wisent-backend-alert-router#token,read:wisent-backend-data-router#token,read:wisent-backend-inactivity-webhook#secret,read:wisent-backend-media-router#token,read:wisent-backend-model-router#token,read:wisent-backend-object-client#token,read:wisent-backend-object-signing#key,read:wisent-backend-release-runner#token,read:wisent-backend-scheduler#token,read:wisent-trade-agent-email#token,read:wisent-trade-agent-model-router#token'

[ -x "$skarbiec" ] || { printf '%s\n' "missing Skarbiec binary: $skarbiec" >&2; exit 1; }
[ -f "$vault" ] || { printf '%s\n' "missing Skarbiec vault: $vault" >&2; exit 1; }
[ -f "$grant" ] || { printf '%s\n' "missing local-agent token: $grant" >&2; exit 1; }
[ -f "$hf_source" ] || { printf '%s\n' "missing staged Hugging Face token: $hf_source" >&2; exit 1; }

python3 -c 'import json,sys; print(json.dumps({"schema":"skarbiec.item.v2","kind":"token","fields":{"token":sys.stdin.read().rstrip("\r\n")},"context":{}}))' \
  < "$hf_source" |
  SKARBIEC_VAULT_FILE="$vault" "$skarbiec" set-json stado-huggingface --type token

SKARBIEC_VAULT_FILE="$vault" "$skarbiec" token-mint stado-local-agent \
  --capabilities "$capabilities" \
  --replace-capabilities \
  --token-file "$grant" >/dev/null
printf '%s\n' "updated stado-local-agent exact-field grant"
