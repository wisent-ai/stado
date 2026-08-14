#!/bin/sh
set -eu

skarbiec="$HOME/.stado/bin/skarbiec"
vault="$HOME/.stado/skarbiec.vault.json"
grant="$HOME/.stado/local-agent-skarbiec-token"
capabilities='read:compute-marketplace-agent#token,read:jeden-model-router#token,read:jeden-agent-auth#agent_auth_secret,read:stado-huggingface#token,read:probierz-model-router#token,read:probierz-agent-auth#agent_auth_secret,read:trading-autonomy-agent-auth#token,read:trading-autonomy-media-router#token,read:trading-autonomy-model-router#token,read:wisent-backend-alert-router#token,read:wisent-backend-data-router#token,read:wisent-backend-inactivity-webhook#secret,read:wisent-backend-media-router#token,read:wisent-backend-model-router#token,read:wisent-backend-object-client#token,read:wisent-backend-object-signing#key,read:wisent-backend-release-runner#token,read:wisent-backend-scheduler#token,read:wisent-trade-agent-email#token,read:wisent-trade-agent-model-router#token'

[ -x "$skarbiec" ] || { printf '%s\n' "missing Skarbiec binary: $skarbiec" >&2; exit 1; }
[ -f "$vault" ] || { printf '%s\n' "missing Skarbiec vault: $vault" >&2; exit 1; }
[ -f "$grant" ] || { printf '%s\n' "missing local-agent token: $grant" >&2; exit 1; }

SKARBIEC_VAULT_FILE="$vault" "$skarbiec" token-mint stado-local-agent \
  --capabilities "$capabilities" \
  --replace-capabilities \
  --token-file "$grant"
