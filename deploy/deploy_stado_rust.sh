#!/bin/bash
# Provider-neutral Rust control-plane deploy.
#
# Release publication is owned by the Azure GitHub workflow. This script runs
# on the coordinator host, installs an optional already-built release artifact,
# then delegates all persistent service rendering to `stado bootstrap --local`.
# It never provisions cloud resources and never consults gcloud, gsutil, ADC,
# Python deployment code, or a Cloud Run image.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="${STADO_INSTALL_DIR:-$HOME/.stado/bin}"
RELEASE_DIR="${STADO_RELEASE_DIR:-}"

if [ -n "$RELEASE_DIR" ]; then
    if [ ! -x "$RELEASE_DIR/stado" ]; then
        echo "FATAL: STADO_RELEASE_DIR=$RELEASE_DIR has no executable Rust stado binary"
        false
    fi
    mkdir -p "$INSTALL_DIR"
    for name in stado wc stado-coverage stado-fix stado-watchdog stado-mcp; do
        [ -f "$RELEASE_DIR/$name" ] || continue
        cp "$RELEASE_DIR/$name" "$INSTALL_DIR/$name.new"
        chmod u=rwx,go= "$INSTALL_DIR/$name.new"
        mv "$INSTALL_DIR/$name.new" "$INSTALL_DIR/$name"
    done
fi

STADO_BIN="${STADO_BIN:-$INSTALL_DIR/stado}"
if [ ! -x "$STADO_BIN" ]; then
    echo "FATAL: Rust stado binary unresolved; set STADO_BIN or STADO_RELEASE_DIR"
    false
fi
export STADO_BIN

echo "Deploying Rust Stado from the explicitly selected deployment profile."
echo "Preflight fails closed on unresolved active storage, replica, identity,"
echo "release, object auth, networking or quota; fenced providers are never contacted."
exec "$SCRIPT_DIR/install_macos_coordinator.sh"
