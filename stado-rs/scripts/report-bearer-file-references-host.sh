#!/bin/sh
# Which units or configs on this host name the worker-agent bearer file.
#
# A bearer file beside the vault that does not hash to what the vault recorded is
# the trap this session already fell into once: the safe grant path refuses on it,
# and it looks like a credential while authorising nothing. Before removing one,
# the question is whether anything here reads that path.
#
# Read-only, and it prints file paths only: never a token.
set -eu

needle=local-agent-skarbiec-token

printf 'FILE\n'
if [ -e "$HOME/.stado/$needle" ]; then
  printf '%s\t%s bytes\n' "$HOME/.stado/$needle" "$(stat -f %z "$HOME/.stado/$needle" 2>/dev/null || stat -c %s "$HOME/.stado/$needle")"
else
  printf '%s\tabsent\n' "$HOME/.stado/$needle"
fi

printf '\nUNITS_NAMING_IT\n'
found=""
for root in "$HOME/Library/LaunchAgents" /Library/LaunchDaemons /Library/LaunchAgents /etc/systemd/system "$HOME/.config/systemd/user"; do
  [ -d "$root" ] || continue
  hits=$(grep -rl "$needle" "$root" 2>/dev/null || true)
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits"
    found=yes
  fi
done
[ -n "$found" ] || printf 'none\n'

printf '\nENV_FILES_NAMING_IT\n'
hits=$(grep -rl "$needle" "$HOME/.stado"/*.env "$HOME/.stado/files"/*.env "$HOME/.config/stado" 2>/dev/null || true)
if [ -n "$hits" ]; then printf '%s\n' "$hits"; else printf 'none\n'; fi
