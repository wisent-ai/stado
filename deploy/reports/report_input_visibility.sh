#!/usr/bin/env bash
# Report whether this host can see a release input, and through which store.
#
# A release job failed with `input input-skarbiec is absent` for an object the
# operator machine had just listed as present. Both statements can be true: the
# publisher and the builder each resolve `stado://` through their own configured
# coordinates. Asking the builder's own binary is the only way to know what the
# builder sees, so this runs there instead of inferring from the laptop.
#
# Read-only: one stat and the configured coordinates, no secret values.
set -euo pipefail

# `run-helper` passes no arguments -- only correlation UUIDs -- so the URI under
# investigation is the default and an operator overrides it by env when needed.
uri="${RELEASE_INPUT_URI:-stado://sources/skarbiec/5bfafcd74f92cc0d9305bda6ff08e2b1a85008952f4b937fb45845506e6350db/source.tar.gz}"
stado="${STADO_BIN:-$HOME/.stado/bin/stado}"

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'stado %s\n' "$([ -x "$stado" ] && echo "$stado" || echo MISSING)"
[ -x "$stado" ] || exit 0

/usr/bin/env python3 - "${STADO_CONFIG:-$HOME/.config/stado/config.json}" <<'PY' 2>/dev/null || true
import json, sys

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        data = json.load(handle)
except OSError:
    print("config unreadable")
    raise SystemExit
api = (data.get("api") or {}).get("url")
stado = (data.get("storage") or {}).get("stado") or {}
print(f"api_url {api}")
print(f"queue_store_url {stado.get('url')}")
print(f"queue_store_namespace {stado.get('namespace')}")
PY


# `storage stat` presents no credential for release-governed namespaces, so it
# answers `absent` for an object it merely may not read. The disk under the store
# root does not lie, and it separates "not published" from "not authorized" and
# from "published into a different store".
printf -- '--- on disk ---\n'
/usr/bin/env python3 - "${STADO_CONFIG:-$HOME/.config/stado/config.json}" "$uri" <<'PY' 2>/dev/null || true
import json, os, sys

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        data = json.load(handle)
except OSError:
    print("config unreadable")
    raise SystemExit
root = ((data.get("storage") or {}).get("local") or {}).get("path") or ""
root = os.path.expanduser(root)
rest = sys.argv[2].removeprefix("stado://")
candidates = [
    os.path.join(root, "ecosystem", rest),
    os.path.join(root, rest),
]
print(f"store_root {root or 'unresolved'}")
for candidate in candidates:
    if os.path.isfile(candidate):
        print(f"present {os.path.getsize(candidate)} {candidate}")
        break
else:
    for candidate in candidates:
        print(f"missing {candidate}")
PY
printf -- '--- stat ---\n'
"$stado" storage stat "$uri" 2>&1 | sed -n '1,12p'
