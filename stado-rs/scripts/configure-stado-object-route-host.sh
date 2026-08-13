#!/bin/sh
set -eu

[ "$(uname -s)" = Linux ] || {
  printf '%s\n' "Stado object route configuration requires systemd" >&2
  exit 1
}

environment=$(/bin/systemctl show wisent-agent.service --property=Environment --value)
current=
for assignment in $environment
do
  case "$assignment" in
    STADO_API_URL=*) current=${assignment#STADO_API_URL=} ;;
  esac
done
[ -n "$current" ] || {
  printf '%s\n' "wisent-agent.service has no STADO_API_URL" >&2
  exit 1
}

python_bin=$(command -v python3 || true)
[ -n "$python_bin" ]
route=$("$python_bin" - "$current" <<'PY'
import sys
from urllib.parse import urlsplit, urlunsplit

current = sys.argv[1]
parsed = urlsplit(current)
if parsed.scheme != "https" or not parsed.hostname or parsed.path not in ("", "/"):
    raise SystemExit(f"refusing unexpected Stado API URL: {current!r}")
host = parsed.hostname
if ":" in host and not host.startswith("["):
    host = f"[{host}]"
print(urlunsplit(("https", f"{host}:8765", "", "", "")))
PY
)

unit=wisent-agent.service
dropin_directory="/etc/systemd/system/${unit}.d"
dropin="$dropin_directory/stado-object-route.conf"
temporary="${dropin}.tmp.$$"
/bin/mkdir -p "$dropin_directory"
printf '[Service]\nEnvironment="STADO_API_URL=%s"\nEnvironment="WC_STADO_STORAGE_URL=%s"\n' \
  "$route" "$route" >"$temporary"
/bin/chmod 0644 "$temporary"
/bin/mv "$temporary" "$dropin"
/bin/systemctl daemon-reload
printf '%s\n' "$route"
