#!/bin/sh
# Publish this host's beacon through the current Stado path and show the result.
#
# The unit installed here still carries the GCS era: it exports
# GOOGLE_APPLICATION_CREDENTIALS for project wisent-480400, whose billing is
# detached on purpose, so its publish cannot succeed and exits 0 regardless.
# `stado host publish-beacon` is the replacement; this reports whether it works
# from this host before anything is rewired.
set -eu

PATH="$HOME/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

STADO="$HOME/.stado/bin/stado"
[ -x "$STADO" ] || { printf 'no stado binary at %s\n' "$STADO"; exit 1; }

printf 'stado: %s\n' "$("$STADO" --version 2>/dev/null)"
printf '\n== publish-beacon --help ==\n'
"$STADO" host publish-beacon --help 2>&1 | sed -n '1,12p' | sed 's/^/  /'

printf '\n== publishing ==\n'
if "$STADO" host publish-beacon 2>&1 | tail -20 | sed 's/^/  /'; then
  printf '  publish exit: 0\n'
else
  printf '  publish exit: nonzero\n'
fi
