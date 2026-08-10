#!/bin/sh
set -eu
umask 077
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

skarbiec_bin=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}
stado_bin=${STADO_BIN:-$HOME/.stado/bin/stado}
vault_file=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
token_file=${STADO_CONTROL_PLANE_TOKEN_FILE:-$HOME/.stado/control-plane-skarbiec-token}

for required in "$skarbiec_bin" "$stado_bin" "$vault_file"; do
  [ -e "$required" ] || {
    printf '%s\n' "required control-plane input is missing: $required" >&2
    exit 1
  }
done

stage_dir=$(mktemp -d "$HOME/.stado/.control-plane-host-grant.XXXXXX")
trap 'rm -rf "$stage_dir"' EXIT HUP INT TERM
registry_file="$stage_dir/registry.json"
tokens_file="$stage_dir/tokens.json"
result_file="$stage_dir/result.json"
next_token="$stage_dir/control-plane-skarbiec-token"

"$stado_bin" registry pull >"$registry_file"
SKARBIEC_VAULT_FILE="$vault_file" "$skarbiec_bin" tokens >"$tokens_file"

capabilities=$(jq -ner '
  [
    ($tokens[0][]
      | select(.consumer == "stado-control-plane")
      | .capabilities[]
      | .action + ":" + .item + (if .field == null then "" else "#" + .field end)),
    ($registry[0].targets[]
      | select(.ssh != null and .ssh != "")
      | "read:stado-ssh-" + .name + "#private_key")
  ]
  | unique
  | if length == 0 then error("empty control-plane capability set") else join(",") end
' --slurpfile tokens "$tokens_file" --slurpfile registry "$registry_file" /dev/null)

SKARBIEC_VAULT_FILE="$vault_file" "$skarbiec_bin" token-mint stado-control-plane \
  --capabilities "$capabilities" --replace-capabilities >"$result_file"
jq -er '.token | select(type == "string" and length > 0)' "$result_file" >"$next_token"
chmod 600 "$next_token"
mv "$next_token" "$token_file"
printf '%s\n' "stado-control-plane host-key grant reconciled -> $token_file"
