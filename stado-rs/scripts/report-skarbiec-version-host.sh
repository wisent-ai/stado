#!/bin/sh
# Which skarbiec binary does this host run, and does its token-mint take a bearer?
#
# `--token-file` is the whole safety story of a grant edit: it means "keep this
# bearer" instead of "issue a new one nobody has". A binary that predates the flag
# accepts the argument silently, mints a random bearer, and returns it in stdout --
# which is how a rotation ends with the vault on a token no host holds.
#
# Read-only: version, path, and whether the flag appears in the binary's own
# usage text. No vault is loaded and no token is touched.
set -eu

for candidate in "$HOME/.stado/bin/skarbiec" /usr/local/bin/skarbiec "$HOME/.local/bin/skarbiec"; do
  [ -x "$candidate" ] || continue
  printf 'BINARY\t%s\n' "$candidate"
  printf 'VERSION\t%s\n' "$("$candidate" version 2>&1 | tr -d '\n' | cut -c1-200)"
  printf 'MINT_USAGE\t%s\n' "$("$candidate" token-mint 2>&1 | tr -d '\n' | cut -c1-200)"
  printf 'HAS_TOKEN_FILE_FLAG\t'
  if strings "$candidate" 2>/dev/null | grep -qx 'token-file'; then printf 'yes\n'; else printf 'no\n'; fi
  printf '\n'
done
