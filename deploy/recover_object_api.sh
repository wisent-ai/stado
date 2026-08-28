#!/usr/bin/env bash
# Restore the Stado object API when its own client route is unavailable.
#
# The listener is the authority for stado:// objects, so it must read the
# physical local store directly. If it inherits the operator profile's
# `storage.backend=stado`, startup calls its own unopened port and launchd
# loops forever. This helper gives the system daemon an explicit local backend,
# preserves the prior plist, and only unloads a drifted job after proving that
# its loopback health endpoint is already unavailable.
set -euo pipefail

label="com.wisent.always-on.stado-object-api"
plist="/Library/LaunchDaemons/$label.plist"
program="$HOME/.stado/services/$label/current/darwin-arm/stado"
config="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
work="$HOME/.stado/work/object-api-recovery"
log="$HOME/.stado/logs/$label.log"
health="http://127.0.0.1:8765/healthz"

if [ "$(/usr/bin/uname -s)" != "Darwin" ]; then
  printf 'unsupported_os %s\n' "$(/usr/bin/uname -s)" >&2
  exit 65
fi
if [ ! -x "$program" ]; then
  printf 'program_missing %s\n' "$program" >&2
  exit 66
fi

store="$HOME/.stado/local-storage"
if [ -r "$config" ]; then
  configured=$(/usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
value = ((document.get("storage") or {}).get("local") or {}).get("path") or ""
print(os.path.abspath(os.path.expanduser(value)) if value else "")
PY
)
  if [ -n "$configured" ]; then store="$configured"; fi
fi
if [ ! -d "$store" ] || [ ! -r "$store/registry.json" ]; then
  printf 'local_store_missing %s\n' "$store" >&2
  exit 67
fi

/bin/mkdir -p "$work" "$HOME/.stado/logs"
/bin/chmod 700 "$work" "$HOME/.stado/logs"
/usr/bin/touch "$log"
/bin/chmod 600 "$log"
staged=$(/usr/bin/mktemp "$work/$label.plist.XXXXXX")
trap '/bin/rm -f "$staged"' EXIT HUP INT TERM
account=$(/usr/bin/id -un)

/usr/bin/python3 - "$staged" "$label" "$program" "$store" "$account" "$log" "$HOME" "$config" <<'PY'
import plistlib, sys

path, label, program, store, account, log, home, config = sys.argv[1:]
document = {
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
        "HOME": home,
        "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "STADO_CONFIG": config,
        "GNUPGHOME": f"{home}/.gnupg",
        "SKARBIEC_VAULT_FILE": f"{home}/.stado/skarbiec.vault.json",
        "WC_OBJECT_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-object-api-verifier-skarbiec-token",
        "WC_RELEASE_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-release-api-verifier-skarbiec-token",
        "WC_STORAGE_BACKEND": "local",
        "WC_LOCAL_STORAGE_PATH": store,
    },
    "RunAtLoad": True,
    "KeepAlive": True,
    "UserName": account,
    "StandardOutPath": log,
    "StandardErrorPath": log,
}
with open(path, "wb") as handle:
    plistlib.dump(document, handle, fmt=plistlib.FMT_XML, sort_keys=False)
PY
/usr/bin/plutil -lint "$staged" >/dev/null

healthy=0
if /usr/bin/curl -fsS --max-time 3 "$health" 2>/dev/null |
  /usr/bin/grep -Eq '"object"[[:space:]]*:[[:space:]]*true'; then healthy=1; fi
same=0
if /usr/bin/python3 - "$staged" "$plist" <<'PY'
import plistlib, sys
try:
    with open(sys.argv[1], "rb") as expected, open(sys.argv[2], "rb") as actual:
        same = plistlib.load(expected) == plistlib.load(actual)
except (OSError, plistlib.InvalidFileException):
    same = False
raise SystemExit(0 if same else 1)
PY
then same=1; fi

if [ "$healthy" -eq 1 ] && [ "$same" -eq 1 ]; then
  printf 'already_healthy %s store=%s\n' "$label" "$store"
  exit 0
fi

stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
backup="$work/$label.plist.before-$stamp"
if /usr/bin/sudo -n /bin/test -f "$plist"; then
  /usr/bin/sudo -n /bin/cp "$plist" "$backup"
  /usr/bin/sudo -n /usr/sbin/chown "$account" "$backup"
  /bin/chmod 600 "$backup"
  printf 'backup %s\n' "$backup"
fi
if [ "$healthy" -eq 1 ]; then
  # Persist the corrected definition without cycling a serving process. The
  # current launchd job keeps running; the file is authoritative after reboot.
  /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
  printf 'persisted_while_healthy %s store=%s backup=%s loaded_job=unchanged\n' \
    "$label" "$store" "${backup:-none}"
  exit 0
fi


if [ "$same" -eq 1 ]; then
  /usr/bin/sudo -n /bin/launchctl kickstart -k "system/$label"
  action=kickstarted
else
  /usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
  /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
  /usr/bin/sudo -n /bin/launchctl enable "system/$label" >/dev/null 2>&1 || true
  /usr/bin/sudo -n /bin/launchctl bootstrap system "$plist"
  action=reinstalled
fi

attempt=0
while [ "$attempt" -lt 180 ]; do
  if /usr/bin/curl -fsS --max-time 3 "$health" >/dev/null 2>&1; then
    printf '%s %s store=%s backup=%s\n' "$action" "$label" "$store" "${backup:-none}"
    exit 0
  fi
  attempt=$((attempt + 1))
  /bin/sleep 1
done
printf 'health_timeout %s did not answer %s after 180 seconds; backup=%s\n' "$label" "$health" "${backup:-none}" >&2
exit 69
