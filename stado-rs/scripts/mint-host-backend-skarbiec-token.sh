#!/bin/sh
set -eu

service_env=${BRAMA_SERVICE_ENV_FILE:-$HOME/.config/brama/service.env}
if [ -f "$service_env" ]; then
  set -a
  . "$service_env"
  set +a
fi
runtime_dir=${BRAMA_RUNTIME_DIR:-$HOME/.stado/run/brama}
release_marker=${BRAMA_RELEASE_MARKER:-$HOME/.stado/brama-release-version}
IFS= read -r release_version < "$release_marker"
case "$release_version" in
  ''|*[![:alnum:]._-]*)
    printf '%s\n' 'invalid Brama release marker' >/dev/stderr
    false
    ;;
esac
skarbiec_bin="$HOME/.stado/services/brama/releases/$release_version/linux-x86_64/bin/skarbiec"
vault_file="$runtime_dir/vault.json"
token_file="$HOME/.stado/wisent-backend-api-service-deployer-skarbiec-token"
if [ ! -x "$skarbiec_bin" ] || [ ! -f "$vault_file" ]; then
  printf '%s\n' 'Brama Skarbiec runtime is not materialized' >/dev/stderr
  false
fi

export GNUPGHOME="$runtime_dir/gnupg"
export SKARBIEC_VAULT_FILE="$vault_file"
export SKARBIEC_AUDIT_FILE="$runtime_dir/audit.jsonl"
scopes='read:wisent-backend-api-runtime,read:wisent-backend-object-client,read:wisent-backend-object-signing,read:wisent-backend-model-router,read:wisent-backend-media-router,read:wisent-backend-transcription-router,read:needher-generation-worker,read:wisent-backend-inactivity-webhook,read:wisent-backend-fcm,read:wisent-backend-apns,read:wisent-backend-supabase,read:wisent-backend-integration-api,read:wisent-backend-admin-integration-api'
token_tmp=$(mktemp "$HOME/.stado/.wisent-backend-token.XXXXXX")
trap 'rm -f "$token_tmp"' EXIT HUP INT TERM
"$skarbiec_bin" token-mint wisent-backend-api-service-deployer --scopes "$scopes" \
  | python3 -c 'import json,sys; token=json.load(sys.stdin).get("token"); assert isinstance(token,str) and token; sys.stdout.write(token+"\n")' \
  > "$token_tmp"
chmod u=rw,go= "$token_tmp"
mv "$token_tmp" "$token_file"
trap - EXIT HUP INT TERM
printf '%s\n' "$token_file"
