#!/bin/sh
# Repoint live Wisent media locators away from the dead Cloud Storage host.
#
# The iOS client reads `imageUrl` / `videoUrl` straight out of the product
# database, so the column value is the delivery contract for every installed
# client. Rows still naming `storage.googleapis.com/wisent-images-bucket` are
# unreachable: that project is billing-detached on purpose and answers 403.
#
# MODE=dry-run (default) reports what would change and writes the before-image.
# MODE=apply performs the update row by row and re-reads each row to confirm.
# The before-image is always written first, so a rewrite can be reversed with
# the same file it produced.
set -eu

TOKEN_FILE=${WISENT_BACKEND_SKARBIEC_TOKEN_FILE:-$HOME/.stado/wisent-backend-api-service-deployer-skarbiec-token}
SKARBIEC_URL=${WISENT_BACKEND_SKARBIEC_URL:-http://127.0.0.1:8895}
CONSUMER=${WISENT_BACKEND_SKARBIEC_CONSUMER:-wisent-backend-api-service-deployer}
MODE=${MODE:-dry-run}
SOURCE_PREFIX=${SOURCE_PREFIX:-https://storage.googleapis.com/wisent-images-bucket/}
TARGET_PREFIX=${TARGET_PREFIX:?TARGET_PREFIX is required}
BEFORE_IMAGE=${BEFORE_IMAGE:-$HOME/.stado/wisent-media-locators-before.json}
[ -s "$TOKEN_FILE" ]

TOKEN_FILE=$TOKEN_FILE SKARBIEC_URL=$SKARBIEC_URL CONSUMER=$CONSUMER MODE=$MODE \
SOURCE_PREFIX=$SOURCE_PREFIX TARGET_PREFIX=$TARGET_PREFIX BEFORE_IMAGE=$BEFORE_IMAGE \
/usr/bin/python3 - <<'PY'
import datetime
import json
import os
import urllib.error
import urllib.parse
import urllib.request

ITEM = "wisent-backend-supabase"
MODE = os.environ["MODE"]
SOURCE_PREFIX = os.environ["SOURCE_PREFIX"]
TARGET_PREFIX = os.environ["TARGET_PREFIX"]
BEFORE_IMAGE = os.environ["BEFORE_IMAGE"]
CONTRACTS = (
    ("Character", "id", ("imageUrl", "videoUrl")),
    ("ProfilePublic", "id", ("imageUrl",)),
    ("Room", "id", ("imageUrl",)),
)


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
key = next(
    (
        resolved[k]
        for k in ("service_role_key", "supabase_key", "SUPABASE_KEY", "SUPABASE_SERVICE_ROLE_KEY")
        if resolved.get(k)
    ),
    None,
)
if not url or not key:
    raise SystemExit("wisent-backend-supabase exposes no usable URL/key pair")

BASE = url.rstrip("/") + "/rest/v1/"
HEADERS = {"Authorization": "Bearer " + key, "apikey": key, "Content-Type": "application/json"}


def request(method, path, body=None, extra=None):
    headers = dict(HEADERS)
    if extra:
        headers.update(extra)
    req = urllib.request.Request(
        BASE + path, data=json.dumps(body).encode() if body is not None else None,
        headers=headers, method=method,
    )
    with urllib.request.urlopen(req, timeout=60) as response:
        raw = response.read()
        return json.loads(raw) if raw else []


def fetch_rows(table, id_field, columns):
    rows, start = [], 0
    select = ",".join((id_field,) + columns)
    while True:
        page = None
        req = urllib.request.Request(
            BASE + f"{urllib.parse.quote(table, safe='')}?" + urllib.parse.urlencode({"select": select}),
            headers=dict(HEADERS, **{"Range": f"{start}-{start + 999}", "Range-Unit": "items"}),
        )
        with urllib.request.urlopen(req, timeout=60) as response:
            page = json.load(response)
        rows.extend(page)
        if len(page) < 1000:
            return rows
        start += len(page)


planned, before = [], []
for table, id_field, columns in CONTRACTS:
    for row in fetch_rows(table, id_field, columns):
        for column in columns:
            value = row.get(column)
            if not isinstance(value, str) or not value.startswith(SOURCE_PREFIX):
                continue
            key_name = value[len(SOURCE_PREFIX):]
            target = TARGET_PREFIX + key_name
            planned.append({"table": table, "id_field": id_field, "id": row[id_field],
                            "column": column, "before": value, "after": target})

before_document = {
    "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "source_prefix": SOURCE_PREFIX,
    "target_prefix": TARGET_PREFIX,
    "row_count": len(planned),
    "rows": planned,
}
with open(BEFORE_IMAGE, "w", encoding="utf-8") as handle:
    json.dump(before_document, handle, indent=2, sort_keys=True)
    handle.write("\n")

summary = {}
for change in planned:
    label = f"{change['table']}.{change['column']}"
    summary[label] = summary.get(label, 0) + 1
print(json.dumps({"mode": MODE, "planned": len(planned), "by_column": summary,
                  "before_image": BEFORE_IMAGE}, separators=(",", ":")))
if MODE != "apply":
    raise SystemExit(0)

updated, failed = 0, []
for change in planned:
    path = (
        f"{urllib.parse.quote(change['table'], safe='')}?"
        + urllib.parse.urlencode({change["id_field"]: f"eq.{change['id']}"})
    )
    try:
        result = request("PATCH", path, {change["column"]: change["after"]},
                         {"Prefer": "return=representation"})
    except urllib.error.HTTPError as error:
        failed.append({"id": change["id"], "column": change["column"],
                       "error": f"HTTP {error.code}: {error.read().decode(errors='replace')[:200]}"})
        continue
    stored = (result[0] if isinstance(result, list) and result else {}).get(change["column"])
    if stored != change["after"]:
        failed.append({"id": change["id"], "column": change["column"],
                       "error": f"row did not take the new value: {stored!r}"})
        continue
    updated += 1

print(json.dumps({"updated": updated, "failed": len(failed), "failures": failed[:5]},
                 separators=(",", ":")))
raise SystemExit(1 if failed else 0)
PY
