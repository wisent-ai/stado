#!/bin/sh
# Register the `backend` integration provider domain in this host's Stado config.
#
# The product backend verifies every user JWT through Stado's integration API
# (`admin-jwt.verify`). That handler needs a provider grant for the `backend`
# domain and the item holding the Supabase JWKS coordinates. Neither was
# configured, so the handler answered 503 integration_unavailable and every
# authenticated call into the AI service failed with "Invalid or expired JWT
# token". The grant and item now exist; this declares them.
set -eu

config=$HOME/.config/stado/config.json
token_file=$HOME/.stado/backend-provider-skarbiec-token

if [ ! -f "$config" ]; then
    printf '%s\n' "no stado config at $config" >&2
    exit 1
fi
if [ ! -f "$token_file" ]; then
    printf '%s\n' "no provider token at $token_file" >&2
    exit 1
fi

/usr/bin/python3 - "$config" <<'PY'
import json
import shutil
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    config = json.load(handle)

providers = config.setdefault("integration", {}).setdefault("providers", {})
providers["backend"] = {
    "consumer": "stado-backend-integration-provider",
    "token_file": "~/.stado/backend-provider-skarbiec-token",
    "items": [
        "echo-wisent-backend-data-provider",
        "wisent-backend-admin-jwt-provider",
    ],
}

shutil.copyfile(path, path + ".before-backend-provider")
with open(path, "w", encoding="utf-8") as handle:
    json.dump(config, handle, indent=2, sort_keys=True)
    handle.write("\n")
print("domains:", ",".join(sorted(providers)))
PY

/bin/rm -f "$0"
