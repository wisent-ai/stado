#!/bin/sh
set -eu

python3 - "$HOME/.config/stado/config.json" "$HOME/.stado/wisent-backend-api.env" <<'PY'
import json
import pathlib
import sys

config_path = pathlib.Path(sys.argv[1])
env_path = pathlib.Path(sys.argv[2])
config = json.loads(config_path.read_text(encoding="utf-8")) if config_path.is_file() else {}
storage = config.get("storage", {})
stado = storage.get("stado", {})
local = storage.get("local", {})
print(f"config={config_path if config_path.is_file() else 'missing'}")
print(f"backend={storage.get('backend', '')}")
print(f"stado_url={stado.get('url', '')}")
print(f"local_path={local.get('path', '')}")
api_url = ""
if env_path.is_file():
    for line in env_path.read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition("=")
        if separator and key.strip() == "STADO_API_URL":
            api_url = value.strip()
            break
print(f"service_api_url={api_url}")
PY
