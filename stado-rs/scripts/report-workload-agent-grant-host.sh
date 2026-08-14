#!/bin/sh
# Report active workload-grant identity and token-file integrity without values.
set -eu

environment_file="$HOME/.stado/files/stado-agent-grant.env"
[ -s "$environment_file" ]
for name in WC_AGENT_SKARBIEC_URL WC_AGENT_SKARBIEC_CONSUMER WC_AGENT_SKARBIEC_TOKEN_FILE
do
  assignment=$(/usr/bin/grep -m1 "^${name}=" "$environment_file")
  printf '%s\n' "$assignment"
done
token_file=$(/usr/bin/grep -m1 '^WC_AGENT_SKARBIEC_TOKEN_FILE=' "$environment_file" | /usr/bin/cut -d= -f2-)
[ -s "$token_file" ]
/usr/bin/stat --printf='token mode=%a bytes=%s path=%n\n' "$token_file"
/usr/bin/sha256sum "$token_file" | /usr/bin/cut -d' ' -f1
url=$(/usr/bin/grep -m1 '^WC_AGENT_SKARBIEC_URL=' "$environment_file" | /usr/bin/cut -d= -f2-)
consumer=$(/usr/bin/grep -m1 '^WC_AGENT_SKARBIEC_CONSUMER=' "$environment_file" | /usr/bin/cut -d= -f2-)
URL=$url CONSUMER=$consumer TOKEN_FILE=$token_file /usr/bin/python3 - <<'PY'
import json
import os
import urllib.error
import urllib.request

token = open(os.environ["TOKEN_FILE"], encoding="utf-8").read().strip()
for item, field in (
    ("jeden-model-router", "token"),
    ("jeden-agent-auth", "agent_auth_secret"),
    ("stado-huggingface", "token"),
):
    request = urllib.request.Request(
        os.environ["URL"].rstrip("/") + "/v1/items/read",
        data=json.dumps({"id": item, "field": field}).encode(),
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "X-Consumer": os.environ["CONSUMER"],
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            value = json.load(response).get("value")
            print(f"{item}#{field}: HTTP {response.status}, length={len(value) if isinstance(value, str) else 0}")
    except urllib.error.HTTPError as error:
        print(f"{item}#{field}: HTTP {error.code}, {error.read().decode(errors='replace')[:200]}")
PY
/bin/systemctl show wisent-agent.service --property=MainPID,ActiveState,SubState --no-pager
