#!/bin/sh
# Register the configured workload-agent bearer against canonical Skarbiec.
set -eu

PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin
export PATH
config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}
vault=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
stado=${STADO_BIN:-$HOME/.stado/bin/stado}
target_file=${STADO_AGENT_GRANT_TARGET_FILE:-$HOME/.stado/files/stado-agent-grant.target}
mode=preserve
target=
if [ -s "$target_file" ]; then
  mode=rotate-and-install
  target=$(/usr/bin/tr -d '\r\n' <"$target_file")
  case "$target" in
    ''|*[!A-Za-z0-9._-]*)
      printf 'invalid workload grant target in %s\n' "$target_file" >&2
      exit 2
      ;;
  esac
fi
desired=${STADO_AGENT_CONFIG_SOURCE:-$HOME/.stado/files/stado.config.canonical.json}
if [ -f "$desired" ]; then
  next=$(/usr/bin/mktemp "$HOME/.config/stado/config.XXXXXX")
  trap '/bin/rm -f "$next"' EXIT HUP INT TERM
  /usr/bin/jq --slurpfile desired "$desired" \
    '.agent.skarbiec = $desired[0].agent.skarbiec' "$config" >"$next"
  /bin/chmod 600 "$next"
  /bin/mv "$next" "$config"
  trap - EXIT HUP INT TERM
fi
consumer=$(/usr/bin/jq -er '.agent.skarbiec.consumer | select(type == "string" and length > 0)' "$config")
token_file=$(/usr/bin/jq -er '.agent.skarbiec.token_file | select(type == "string" and length > 0)' "$config")
case "$token_file" in
  '~/'*) token_file="$HOME/${token_file#\~/}" ;;
esac
if [ "$mode" = preserve ]; then
  [ -s "$token_file" ] || { printf 'missing workload grant: %s\n' "$token_file" >&2; exit 1; }
fi
capabilities=$(/usr/bin/jq -er '.agent.skarbiec.secret_fields | map("read:" + .) | join(",") | select(length > 0)' "$config")

export SKARBIEC_VAULT_FILE=$vault
result=$(/usr/bin/mktemp "$HOME/.stado/workload-agent-grant.XXXXXX")
trap '/bin/rm -f "$result"' EXIT HUP INT TERM
active_token_file=$token_file
if [ "$mode" = preserve ]; then
  "$skarbiec" token-mint "$consumer" \
    --capabilities "$capabilities" \
    --token-file "$token_file" \
    --replace-capabilities >"$result"
  /usr/bin/jq -e --arg consumer "$consumer" \
    '.ok == true and .consumer == $consumer and .token == null' "$result" >/dev/null
else
  staged_directory=$HOME/.stado/staged-grants
  staged_token=$staged_directory/$consumer.token
  /bin/mkdir -p "$staged_directory"
  /bin/chmod 700 "$staged_directory"
  if [ ! -s "$staged_token" ]; then
    "$skarbiec" token-mint "$consumer" \
      --capabilities "$capabilities" \
      --replace-capabilities >"$result"
    /usr/bin/jq -er --arg consumer "$consumer" \
      '. as $grant | select($grant.ok == true and $grant.consumer == $consumer) | .token | select(type == "string" and length > 0)' \
      "$result" >"$staged_token"
    /bin/chmod 600 "$staged_token"
  fi
  active_token_file=$staged_token
fi

CONSUMER=$consumer TOKEN_FILE=$active_token_file /usr/bin/python3 - <<'PY'
import json
import os
import urllib.request

consumer = os.environ["CONSUMER"]
token = open(os.environ["TOKEN_FILE"], encoding="utf-8").read().strip()
for item, field in (
    ("jeden-model-router", "token"),
    ("jeden-agent-auth", "agent_auth_secret"),
):
    request = urllib.request.Request(
        "http://127.0.0.1:8895/v1/items/read",
        data=json.dumps({"id": item, "field": field}).encode(),
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "X-Consumer": consumer,
        },
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=15) as response:
        value = json.load(response).get("value")
        if not isinstance(value, str) or not value:
            raise SystemExit(f"{item}#{field} resolved empty")
        print(json.dumps({"item": item, "field": field, "length": len(value)}, separators=(",", ":")))
PY
if [ "$mode" = rotate-and-install ]; then
  token_name=$(/usr/bin/basename "$token_file")
  "$stado" host install-secret "$target" "$active_token_file" "$token_name" --json
  /bin/rm -f "$active_token_file"
  active_token_file=
fi

if [ -s "$active_token_file" ]; then
  /usr/bin/shasum -a 256 "$active_token_file"
fi
printf 'workload grant reconciled: consumer=%s capabilities=%s\n' \
  "$consumer" "$(( $(printf '%s' "$capabilities" | /usr/bin/tr -cd ',' | /usr/bin/wc -c) + 1 ))"
