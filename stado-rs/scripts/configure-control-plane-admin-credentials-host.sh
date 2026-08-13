#!/bin/sh
# Bind host-channel key reads to the registry-derived control-plane grant.
set -eu

stado_bin=${STADO_BIN:-$HOME/.stado/bin/stado}
[ -x "$stado_bin" ] || {
  printf '%s\n' "missing Stado binary: $stado_bin" >&2
  exit 1
}

"$stado_bin" config set credentials.admin.consumer stado-control-plane >/dev/null
"$stado_bin" config set credentials.admin.token_file '~/.stado/control-plane-skarbiec-token' >/dev/null
printf '%s\n' 'configured credentials.admin for stado-control-plane'
