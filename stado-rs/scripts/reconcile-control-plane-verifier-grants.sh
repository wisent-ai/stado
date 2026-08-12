#!/bin/sh
set -eu
umask 077
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}

if [ -n "${SKARBIEC_BIN:-}" ]; then
  skarbiec_bin=$SKARBIEC_BIN
else
  skarbiec_bin=
  for candidate in \
    "$HOME/.stado/bin/skarbiec" \
    "$HOME/.local/bin/skarbiec" \
    "$HOME/.stado/services/skarbiec/current/darwin-arm64/bin/skarbiec" \
    "$HOME/.stado/services/skarbiec/current/bin/skarbiec"
  do
    if [ -x "$candidate" ]; then
      skarbiec_bin=$candidate
      break
    fi
  done
fi

if [ -n "${SKARBIEC_VAULT_FILE:-}" ]; then
  vault_file=$SKARBIEC_VAULT_FILE
else
  vault_file=
  for candidate in \
    "$HOME/.stado/skarbiec.vault.json" \
    "$HOME/.local/share/skarbiec/skarbiec.vault.json" \
    "$HOME/.stado/run/brama/vault.json"
  do
    if [ -f "$candidate" ]; then
      vault_file=$candidate
      break
    fi
  done
fi

if [ ! -f "$config" ] || [ -z "$skarbiec_bin" ] || [ -z "$vault_file" ]; then
  printf 'config=%s binary=%s vault=%s\n' "$config" "${skarbiec_bin:-missing}" "${vault_file:-missing}" >&2
  /bin/ps axww -o pid=,command= | /usr/bin/awk '/[s]karbiec/ { print }' >&2
  exit 1
fi

stage_dir=$(mktemp -d "$HOME/.stado/.verifier-grants.XXXXXX")
trap 'rm -rf "$stage_dir"' EXIT HUP INT TERM


mint_verifier() {
  consumer=$1
  token_name=$2
  policy_filter=$3
  extra=${4:-}
  capabilities=$(
    jq -er "[$policy_filter | .item | \"read:\" + . + \"#token\"] | unique | if length == 0 then error(\"empty verifier policy\") else join(\",\") end" "$config"
  )
  [ -z "$extra" ] || capabilities="$capabilities,$extra"
  capability_count=$(jq -er "[$policy_filter | .item] | unique | length" "$config")
  result_file="$stage_dir/$consumer.json"
  token_file="$stage_dir/$token_name"

  SKARBIEC_VAULT_FILE="$vault_file" "$skarbiec_bin" token-mint "$consumer" \
    --capabilities "$capabilities" --replace-capabilities >"$result_file"
  jq -er '.token | select(type == "string" and length > 0)' "$result_file" >"$token_file"
  chmod 600 "$token_file"
  mv "$token_file" "$HOME/.stado/$token_name"
  printf '%s reconciled (%s capabilities) -> %s\n' \
    "$consumer" "$capability_count" "$HOME/.stado/$token_name"
}

mint_verifier stado-object-api-verifier stado-object-api-verifier-skarbiec-token '.object_api.namespaces[]'
mint_verifier stado-release-api-verifier stado-release-api-verifier-skarbiec-token '.release_api.publishers[]'
mint_verifier stado-machine-api-verifier stado-machine-api-verifier-skarbiec-token '.machine_api.clients[]'
mint_verifier stado-service-api-verifier stado-service-api-verifier-skarbiec-token '.service_api.deployers[]'
mint_verifier stado-rate-limit-api-verifier stado-rate-limit-api-verifier-skarbiec-token '.rate_limit.clients[]'
mint_verifier stado-integration-api-verifier stado-integration-api-verifier-skarbiec-token '.integration.clients[]'
mint_verifier stado-backend-push-api-verifier stado-backend-push-api-verifier-skarbiec-token '.backend.push_clients[]'
