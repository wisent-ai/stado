#!/usr/bin/env bash
# Provider-neutral Stado bootstrap entrypoint.
#
# The retired installer downloaded the Python wisent-compute package, searched
# for Google application-default credentials, rendered GCS-bound units, and
# installed a beacon that wrote directly to a bucket. That path is unsupported.
# Install a verified Rust release first, then run this script as the service
# account that will own the launchd or systemd user unit.
#
# Required environment:
#   STADO_CONFIG   resolved Stado configuration file
# Optional:
#   STADO_TARGET   coordinator name; defaults to local-control-plane
#   STADO_BIN      verified Rust binary; defaults to ~/.stado/bin/stado

set -euo pipefail

STADO_TARGET="${STADO_TARGET:-local-control-plane}"
STADO_CONFIG="${STADO_CONFIG:-}"
STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"

if [ "$STADO_TARGET" != "local-control-plane" ]; then
    echo "[install] FATAL: local installation is owned only by local-control-plane; provision remote agents through the coordinator" > /dev/stderr
    false
fi
if [ -z "$STADO_CONFIG" ] || [ ! -r "$STADO_CONFIG" ]; then
    echo "[install] FATAL: STADO_CONFIG must name a readable resolved config" > /dev/stderr
    false
fi
if [ ! -x "$STADO_BIN" ]; then
    echo "[install] FATAL: verified Rust stado binary unavailable at $STADO_BIN" > /dev/stderr
    false
fi

export STADO_CONFIG
"$STADO_BIN" config validate
"$STADO_BIN" doctor --fix-hints
exec "$STADO_BIN" bootstrap --local --target "$STADO_TARGET"
