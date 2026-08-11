#!/bin/sh
set -eu

config=
environment=$(/bin/systemctl show wisent-agent.service --property=Environment --value)
for assignment in $environment
do
  case "$assignment" in
    STADO_CONFIG=*) config=${assignment#STADO_CONFIG=} ;;
  esac
done
for candidate in "$config" "$HOME/.config/stado/config.json" "$HOME/.stado/config.json" "$HOME/.stado/stado.config.json"
do
  if [ -n "$candidate" ] && [ -f "$candidate" ]
  then
    config=$candidate
    break
  fi
done
[ -n "$config" ] && [ -f "$config" ] || {
  printf '%s\n' "Stado agent config file was not found" >&2
  exit 1
}
temporary="${config}.tmp.$$"
python_bin=$(command -v python3 || true)
[ -n "$python_bin" ]
"$python_bin" - "$config" "$temporary" <<'PY'
import json
import sys
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit

source = Path(sys.argv[1])
temporary = Path(sys.argv[2])
document = json.loads(source.read_text(encoding="utf-8"))
stado = document.setdefault("storage", {}).setdefault("stado", {})
current = str(stado.get("url", ""))
parsed = urlsplit(current)
if parsed.scheme != "https" or not parsed.hostname or parsed.path not in ("", "/"):
    raise SystemExit(f"refusing unexpected Stado object URL: {current!r}")
host = parsed.hostname
if ":" in host and not host.startswith("["):
    host = f"[{host}]"
stado["url"] = urlunsplit(("https", f"{host}:8443", "", "", ""))
temporary.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(stado["url"])
PY
/bin/chmod 0600 "$temporary"
/bin/mv "$temporary" "$config"
