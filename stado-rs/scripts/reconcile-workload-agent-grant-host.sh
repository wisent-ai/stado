#!/bin/sh
# Register the configured workload-agent bearer against canonical Skarbiec.
set -eu

PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin
export PATH
config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
skarbiec=${SKARBIEC_BIN:-$HOME/.stado/bin/skarbiec}
vault=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
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
[ -s "$token_file" ] || { printf 'missing workload grant: %s\n' "$token_file" >&2; exit 1; }
capabilities=$(/usr/bin/jq -er '.agent.skarbiec.secret_fields | map("read:" + .) | join(",") | select(length > 0)' "$config")

export SKARBIEC_VAULT_FILE=$vault
result=$(/usr/bin/mktemp "$HOME/.stado/workload-agent-grant.XXXXXX")
trap '/bin/rm -f "$result"' EXIT HUP INT TERM
"$skarbiec" token-mint "$consumer" \
  --capabilities "$capabilities" \
  --token-file "$token_file" \
  --replace-capabilities >"$result"
/usr/bin/jq -e --arg consumer "$consumer" \
  '.ok == true and .consumer == $consumer and .token == null' "$result" >/dev/null

CONSUMER=$consumer TOKEN_FILE=$token_file /usr/bin/python3 - <<'PY'
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

/usr/bin/shasum -a 256 "$token_file"
printf 'workload grant reconciled: consumer=%s capabilities=%s\n' \
  "$consumer" "$(( $(printf '%s' "$capabilities" | /usr/bin/tr -cd ',' | /usr/bin/wc -c) + 1 ))"
