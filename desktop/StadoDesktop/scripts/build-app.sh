#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec zsh "$SCRIPT_DIR/bundle.sh" "$@"
