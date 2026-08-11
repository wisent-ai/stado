#!/bin/sh
set -eu

config="$HOME/.config/stado/config.json"
[ -f "$config" ]
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
