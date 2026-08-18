#!/bin/sh
# Let the browser client redeem the Cloudflare dashboard login.
#
# `fill_credential` builds the capability resource `origin:<page origin>/<field
# class>` and redeems it through the Skarbiec broker. The broker resolves that
# resource in `~/.stado/capability-routes.json`, which carries the Apple
# equivalents (`origin:https://idmsa.apple.com/email` and `/password`) and
# nothing for Cloudflare -- so the agent reached the login form, asked for the
# credential and was told "Authentication credentials not available or invalid",
# while `platform-admin-cloudflare` sat in the vault with `username` and
# `password` the whole time.
#
# Idempotent: existing entries are left exactly as they are, the previous file is
# kept beside the new one, and the result is re-parsed before it replaces
# anything.
set -u

ROUTES="$HOME/.stado/capability-routes.json"
[ -f "$ROUTES" ] || { printf 'missing %s\n' "$ROUTES" >/dev/stderr; exit 1; }
stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)

/usr/bin/python3 - "$ROUTES" "$stamp" <<'PY'
import json, os, shutil, sys, tempfile

path, stamp = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as handle:
    routes = json.load(handle)
if not isinstance(routes, dict):
    raise SystemExit("capability routes is not an object")

wanted = {
    "origin:https://dash.cloudflare.com/email": {
        "item": "platform-admin-cloudflare",
        "field": "username",
    },
    "origin:https://dash.cloudflare.com/password": {
        "item": "platform-admin-cloudflare",
        "field": "password",
    },
}
added = []
for resource, route in wanted.items():
    if resource in routes:
        continue
    routes[resource] = route
    added.append(resource)

if not added:
    print(json.dumps({"added": [], "already_present": sorted(wanted)}))
    raise SystemExit(0)

shutil.copy2(path, f"{path}.before-{stamp}")
directory = os.path.dirname(path)
descriptor, temporary = tempfile.mkstemp(dir=directory)
with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
    json.dump(routes, handle, indent=1, sort_keys=True)
    handle.write("\n")
os.chmod(temporary, 0o600)
# Re-read the written file before it replaces the live one: a truncated write
# would otherwise be published and the broker would stop resolving everything.
with open(temporary, encoding="utf-8") as handle:
    json.load(handle)
os.replace(temporary, path)
print(json.dumps({"added": added, "backup": f"{path}.before-{stamp}"}))
PY

printf 'cloudflare_entries=%s\n' "$(/usr/bin/grep -c 'dash.cloudflare.com' "$ROUTES")"
