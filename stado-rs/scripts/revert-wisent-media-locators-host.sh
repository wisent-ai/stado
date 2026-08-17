#!/bin/sh
# Undo the most recent media locator pass, row by row, from its before-image.
#
# Why this exists rather than another prefix rewrite: a prefix rewrite off
# `bobloo.com` matches 4,833 rows, but only 2,674 of them were ever migrated.
# The other 2,159 named `bobloo.com` long before this migration and their objects
# are not in the Azure container, so rewriting them by prefix would break rows
# that this work never touched. The before-image records exactly which row,
# column and value each pass changed, so it is the only safe reversal.
#
# It reverts the newest before-image that still matches the live values, applies
# `before` to each row, and re-reads every row it writes.
set -eu

TOKEN_FILE=${WISENT_BACKEND_SKARBIEC_TOKEN_FILE:-$HOME/.stado/wisent-backend-api-service-deployer-skarbiec-token}
SKARBIEC_URL=${WISENT_BACKEND_SKARBIEC_URL:-http://127.0.0.1:8895}
CONSUMER=${WISENT_BACKEND_SKARBIEC_CONSUMER:-wisent-backend-api-service-deployer}
MODE=${MODE:-dry-run}
[ -s "$TOKEN_FILE" ]

TOKEN_FILE=$TOKEN_FILE SKARBIEC_URL=$SKARBIEC_URL CONSUMER=$CONSUMER MODE=$MODE \
/usr/bin/python3 - <<'PY'
import glob
import json
import os
import urllib.error
import urllib.parse
import urllib.request

ITEM = "wisent-backend-supabase"
MODE = os.environ["MODE"]


def read_item_field(field):
    token = open(os.environ["TOKEN_FILE"], encoding="utf-8").read().strip()
    request = urllib.request.Request(
        os.environ["SKARBIEC_URL"].rstrip("/") + "/v1/items/read",
        data=json.dumps({"id": ITEM, "field": field}).encode(),
        headers={
            "Authorization": "Bearer " + token,
            "Content-Type": "application/json",
            "X-Consumer": os.environ["CONSUMER"],
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            value = json.load(response).get("value")
    except urllib.error.HTTPError:
        return None
    return value if isinstance(value, str) and value else None


resolved = {}
for field in ("value", "url", "supabase_url", "SUPABASE_URL", "service_role_key",
              "supabase_key", "SUPABASE_KEY", "SUPABASE_SERVICE_ROLE_KEY"):
    value = read_item_field(field)
    if value is not None:
        resolved[field] = value
bundled = resolved.get("value")
if bundled:
    try:
        document = json.loads(bundled)
    except json.JSONDecodeError:
        document = {}
    if isinstance(document, dict):
        for key, value in document.items():
            if isinstance(value, str) and value:
                resolved.setdefault(key, value)

url = next((resolved[k] for k in ("url", "supabase_url", "SUPABASE_URL") if resolved.get(k)), None)
key = next((resolved[k] for k in ("service_role_key", "supabase_key", "SUPABASE_KEY",
                                  "SUPABASE_SERVICE_ROLE_KEY") if resolved.get(k)), None)
if not url or not key:
    raise SystemExit("wisent-backend-supabase exposes no usable URL/key pair")

BASE = url.rstrip("/") + "/rest/v1/"
HEADERS = {"Authorization": "Bearer " + key, "apikey": key, "Content-Type": "application/json"}

# Newest is not good enough: a dry-run writes a before-image too, so the latest
# file on disk can describe a pass that was never applied. Select by what the
# pass was for — the one that moved rows onto the Wisent host — and take the
# newest of those.
REVERTED_TARGET = "https://bobloo.com/"
candidates = []
for path in sorted(glob.glob(os.path.expanduser("~/.stado/wisent-media-locators-before-*.json"))):
    try:
        parsed = json.loads(open(path, encoding="utf-8").read())
    except (OSError, json.JSONDecodeError):
        continue
    if parsed.get("target_prefix") == REVERTED_TARGET and parsed.get("rows"):
        candidates.append((path, parsed))
if not candidates:
    raise SystemExit(f"no before-image records a pass onto {REVERTED_TARGET}")
selected, document = candidates[-1]
images = [selected]
rows = document["rows"]

print(json.dumps({
    "before_image": images[-1],
    "recorded_rows": len(rows),
    "source_prefix": document.get("source_prefix"),
    "target_prefix": document.get("target_prefix"),
}, separators=(",", ":")))

updated, skipped, failed = 0, 0, []
for change in rows:
    table = change["table"]
    id_field = change.get("id_field", "id")
    path = (
        f"{urllib.parse.quote(table, safe='')}?"
        + urllib.parse.urlencode({id_field: f"eq.{change['id']}", "select": f"{id_field},{change['column']}"})
    )
    try:
        with urllib.request.urlopen(urllib.request.Request(BASE + path, headers=HEADERS), timeout=60) as response:
            live = json.load(response)
    except urllib.error.HTTPError as error:
        failed.append({"id": change["id"], "error": f"read HTTP {error.code}"})
        continue
    current = (live[0] if isinstance(live, list) and live else {}).get(change["column"])
    if current != change["after"]:
        skipped += 1
        continue
    if MODE != "apply":
        updated += 1
        continue
    body = {change["column"]: change["before"]}
    write_path = (
        f"{urllib.parse.quote(table, safe='')}?"
        + urllib.parse.urlencode({id_field: f"eq.{change['id']}"})
    )
    try:
        request = urllib.request.Request(
            BASE + write_path, data=json.dumps(body).encode(),
            headers=dict(HEADERS, Prefer="return=representation"), method="PATCH",
        )
        with urllib.request.urlopen(request, timeout=60) as response:
            result = json.load(response)
    except urllib.error.HTTPError as error:
        failed.append({"id": change["id"], "error": f"write HTTP {error.code}"})
        continue
    stored = (result[0] if isinstance(result, list) and result else {}).get(change["column"])
    if stored != change["before"]:
        failed.append({"id": change["id"], "error": "row did not take the reverted value"})
        continue
    updated += 1

print(json.dumps({"mode": MODE, "reverted": updated, "already_changed": skipped,
                  "failed": len(failed), "failures": failed[:5]}, separators=(",", ":")))
raise SystemExit(1 if failed else 0)
PY
