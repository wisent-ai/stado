#!/bin/sh
# Whether the safe re-mint path for one consumer is reachable on this host.
#
# `scripts/grant-consumer-field-read.py` adds a capability without rotating a
# bearer, and it can only do that where two things sit together: the vault, and
# an owner-only token file that still hashes to the bearer the vault recorded. It
# refuses otherwise, on purpose -- a bearer that cannot be reproduced must not be
# replaced, because every consumer authenticating with it would stop.
#
# This reports whether both are here for the fleet's worker-agent consumer.
# Read-only, and no token or hash prefix is printed: presence and sizes only.
set -eu

vault=${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}
printf 'VAULT\t'
if [ -r "$vault" ]; then printf '%s (%s bytes)\n' "$vault" "$(stat -f %z "$vault" 2>/dev/null || stat -c %s "$vault")"; else printf 'absent at %s\n' "$vault"; fi

printf 'SKARBIEC_BIN\t'
if [ -x "$HOME/.stado/bin/skarbiec" ]; then "$HOME/.stado/bin/skarbiec" version 2>&1 | head -n 1; else printf 'absent\n'; fi

printf '\nTOKEN_FILES\n'
for candidate in "$HOME"/.stado/*skarbiec-token*; do
  [ -e "$candidate" ] || continue
  printf '%s\t%s bytes\n' "$(basename "$candidate")" "$(stat -f %z "$candidate" 2>/dev/null || stat -c %s "$candidate")"
done

printf '\nGRANT_SCRIPT_INPUTS\n'
for name in local-agent-skarbiec-token control-plane-skarbiec-token; do
  if [ -r "$HOME/.stado/$name" ]; then printf '%s\tpresent\n' "$name"; else printf '%s\tabsent\n' "$name"; fi
done
