#!/bin/sh
set -eu

target=gpu-host
stado_bin="$HOME/.stado/bin/stado"
if [ ! -x "$stado_bin" ]; then
    echo "target Stado binary is unavailable" >&2
    false
fi
exec "$stado_bin" release agent --target "$target" --once --json
