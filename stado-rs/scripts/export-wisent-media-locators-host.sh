#!/bin/sh
set -eu

TOKEN_FILE=${WISENT_BACKEND_SKARBIEC_TOKEN_FILE:-$HOME/.stado/wisent-backend-api-service-deployer-skarbiec-token}
SKARBIEC_URL=${WISENT_BACKEND_SKARBIEC_URL:-http://127.0.0.1:8895}
CONSUMER=${WISENT_BACKEND_SKARBIEC_CONSUMER:-wisent-backend-api-service-deployer}
[ -s "$TOKEN_FILE" ]

TOKEN_FILE=$TOKEN_FILE SKARBIEC_URL=$SKARBIEC_URL CONSUMER=$CONSUMER /usr/bin/python3 - <<'PY'
import datetime
import json
import os
import urllib.error
import urllib.parse
import urllib.request

SOURCE_PREFIX = "https://storage.googleapis.com/wisent-images-bucket/"
ITEM = "wisent-backend-supabase"


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
for field in (
    "value",
    "url",
    "supabase_url",
    "SUPABASE_URL",
    "service_role_key",
    "supabase_key",
    "SUPABASE_KEY",
    "SUPABASE_SERVICE_ROLE_KEY",
    "anon_key",
    "SUPABASE_ANON_KEY",
):
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

url = next(
    (resolved.get(key) for key in ("url", "supabase_url", "SUPABASE_URL") if resolved.get(key)),
    None,
)
key = next(
    (
        resolved.get(name)
        for name in (
            "service_role_key",
            "supabase_key",
            "SUPABASE_KEY",
            "SUPABASE_SERVICE_ROLE_KEY",
            "anon_key",
            "SUPABASE_ANON_KEY",
        )
        if resolved.get(name)
    ),
    None,
)
if not url or not key:
    raise SystemExit(
        "wisent-backend-supabase does not expose a recognized URL/key contract; "
        "available fields=" + ",".join(sorted(resolved))
    )


def fetch_rows(table, columns):
    rows = []
    start = 0
    while True:
        query = urllib.parse.urlencode({"select": ",".join(columns)})
        request = urllib.request.Request(
            url.rstrip("/") + "/rest/v1/" + urllib.parse.quote(table, safe="") + "?" + query,
            headers={
                "Authorization": "Bearer " + key,
                "apikey": key,
                "Range": f"{start}-{start + 999}",
                "Range-Unit": "items",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                page = json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors="replace")[:1000]
            raise SystemExit(f"{table} query failed with HTTP {error.code}: {detail}") from None
        if not isinstance(page, list):
            raise SystemExit(f"{table} did not return a row list")
        rows.extend(page)
        if len(page) < 1000:
            return rows
        start += len(page)


contracts = (
    ("Character", ("id", "imageUrl", "videoUrl"), "id"),
    ("ProfilePublic", ("id", "imageUrl"), "id"),
    ("Room", ("id", "imageUrl"), "id"),
)
entries = {}
reference_count = 0
for table, columns, id_field in contracts:
    for row in fetch_rows(table, columns):
        row_id = row.get(id_field)
        for field in columns:
            value = row.get(field)
            if field == id_field or not isinstance(value, str) or not value.startswith(SOURCE_PREFIX):
                continue
            key_name = urllib.parse.unquote(value[len(SOURCE_PREFIX):])
            entry = entries.setdefault(
                key_name,
                {
                    "source_uri": "gs://wisent-images-bucket/" + key_name,
                    "object_key": key_name,
                    "references": [],
                },
            )
            entry["references"].append({"table": table, "id": row_id, "field": field})
            reference_count += 1

manifest = {
    "schema_version": 1,
    "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "source_bucket": "wisent-images-bucket",
    "source_prefix": SOURCE_PREFIX,
    "object_count": len(entries),
    "reference_count": reference_count,
    "entries": [entries[name] for name in sorted(entries)],
}
print(json.dumps(manifest, separators=(",", ":"), sort_keys=True))
PY
