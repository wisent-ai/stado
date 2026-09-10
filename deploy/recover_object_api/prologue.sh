#!/usr/bin/env bash
# Restore the Stado object API without trusting a successful read alone.
#
# launchd's loaded job and its plist are separate facts. Recovery proves the
# loaded storage route and authenticated reads before reporting readiness.
# A different authority requires `host storage-root-reconcile`, which owns the
# snapshots, writer fence, namespace-qualified copy, and rollback. This helper
# repairs only the same-root listener using the host's canonical delivered Stado.
set -euo pipefail

label="com.wisent.always-on.stado-object-api"
plist="/Library/LaunchDaemons/$label.plist"
program="$HOME/.stado/bin/stado"
config="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
work="$HOME/.stado/work/object-api-recovery"
log="$HOME/.stado/logs/$label.log"

if [ "$(/usr/bin/uname -s)" != "Darwin" ]; then
  printf 'unsupported_os %s\n' "$(/usr/bin/uname -s)" >&2
  exit 65
fi
if [ ! -x "$program" ]; then
  printf 'program_missing %s\n' "$program" >&2
  exit 66
fi

store="${WC_LOCAL_STORAGE_PATH:-$HOME/.stado/local-storage}"
backup_store="${WC_BACKUP_LOCAL_STORAGE_PATH:-$HOME/.stado/local-backup}"
if [ -r "$config" ]; then
  configured=$(/usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
value = ((document.get("storage") or {}).get("local") or {}).get("path") or ""
print(os.path.realpath(os.path.abspath(os.path.expanduser(value))) if value else "")
PY
)
  if [ -z "${WC_LOCAL_STORAGE_PATH:-}" ] && [ -n "$configured" ]; then
    store="$configured"
  fi
  configured_backup=$(/usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
value = ((((document.get("storage") or {}).get("backup") or {}).get("local") or {}).get("path") or "")
print(os.path.realpath(os.path.abspath(os.path.expanduser(value))) if value else "")
PY
)
  if [ -z "${WC_BACKUP_LOCAL_STORAGE_PATH:-}" ] &&
    [ -n "$configured_backup" ]; then
    backup_store="$configured_backup"
  fi
fi
store=$(/usr/bin/python3 - "$store" <<'PY'
import os, sys
print(os.path.realpath(os.path.abspath(os.path.expanduser(sys.argv[1]))))
PY
)
backup_store=$(/usr/bin/python3 - "$backup_store" <<'PY'
import os, sys
print(os.path.realpath(os.path.abspath(os.path.expanduser(sys.argv[1]))))
PY
)
object_url="http://127.0.0.1:8765"
object_namespace="probierz"
object_token_file="$HOME/.stado/queue-object-api-token"
if [ -r "$config" ]; then
  configured_url=$(/usr/bin/python3 - "$config" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
print((((document.get("storage") or {}).get("stado") or {}).get("url") or ""))
PY
)
  configured_namespace=$(/usr/bin/python3 - "$config" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
print((((document.get("storage") or {}).get("stado") or {}).get("namespace") or ""))
PY
)
  configured_token_file=$(/usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
value = (((document.get("storage") or {}).get("stado") or {}).get("token_file") or "")
print(os.path.abspath(os.path.expanduser(value)) if value else "")
PY
)
  if [ -n "$configured_url" ]; then object_url="$configured_url"; fi
  if [ -n "$configured_namespace" ]; then object_namespace="$configured_namespace"; fi
  if [ -n "$configured_token_file" ]; then object_token_file="$configured_token_file"; fi
fi
if [ ! -d "$store" ] || [ ! -r "$store/registry.json" ]; then
  printf 'local_store_missing %s\n' "$store" >&2
  exit 67
fi
if [ "$backup_store" = "$store" ]; then
  printf 'local_backup_matches_primary %s\n' "$store" >&2
  exit 68
fi
/bin/mkdir -p "$backup_store"
/bin/chmod 700 "$backup_store"

/bin/mkdir -p "$work" "$HOME/.stado/logs"
/bin/chmod 700 "$work" "$HOME/.stado/logs"
/usr/bin/touch "$log"
/bin/chmod 600 "$log"
staged=$(/usr/bin/mktemp "$work/$label.plist.XXXXXX")
trap '/bin/rm -f "$staged"' EXIT HUP INT TERM
account=$(/usr/bin/id -un)

/usr/bin/python3 - "$staged" "$label" "$program" "$store" "$backup_store" "$account" "$log" "$HOME" "$config" "$plist" <<'PY'
import plistlib, sys

path, label, program, store, backup_store, account, log, home, config, installed = sys.argv[1:]
# Recovery owns the executable and required environment, not every launchd
# option. Preserve fields the shared service renderer installed, including
# resource limits, instead of rewriting a healthy unit into a second shape.
try:
    with open(installed, "rb") as handle:
        document = plistlib.load(handle)
except (FileNotFoundError, plistlib.InvalidFileException):
    document = {}
if not isinstance(document, dict):
    document = {}
environment = document.get("EnvironmentVariables")
if not isinstance(environment, dict):
    environment = {}
document.pop("Program", None)
document.update({
    "Label": label,
    "ProgramArguments": [
        program,
        "dashboard",
        "--bind",
        "127.0.0.1",
        "--port",
        "8765",
    ],
    "EnvironmentVariables": {
        **environment,
        "HOME": home,
        "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "STADO_CONFIG": config,
        "GNUPGHOME": f"{home}/.gnupg",
        "SKARBIEC_VAULT_FILE": f"{home}/.stado/skarbiec.vault.json",
        "WC_OBJECT_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-object-api-verifier-skarbiec-token",
        "WC_RELEASE_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-release-api-verifier-skarbiec-token",
        "WC_STORAGE_BACKEND": "local",
        "WC_LOCAL_STORAGE_PATH": store,
        "WC_BACKUP_STORAGE_BACKEND": "local",
        "WC_BACKUP_LOCAL_STORAGE_PATH": backup_store,
    },
    "RunAtLoad": True,
    "KeepAlive": True,
    "UserName": account,
    "StandardOutPath": log,
    "StandardErrorPath": log,
})
with open(path, "wb") as handle:
    plistlib.dump(document, handle, fmt=plistlib.FMT_XML, sort_keys=False)
PY
/usr/bin/plutil -lint "$staged" >/dev/null



