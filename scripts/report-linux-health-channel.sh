#!/bin/sh
# Report whether this host can reach the fleet's health API on its own.
#
# The modern beacon unit expects `/etc/stado/host-health.env` to name the
# endpoint, and that API is loopback-only on the control-plane machine, so a
# remote host publishes through a tunnel or not at all. This states which of
# those exist here and whether any discovered endpoint answers. Endpoints are
# discovered from the host's own files rather than written down here, so this
# script carries no address of its own.
set -eu

PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

ENV_FILE=/etc/stado/host-health.env
FORWARDS="$HOME/.stado/forwards"

printf '== %s ==\n' "$ENV_FILE"
if [ -f "$ENV_FILE" ]; then
  sed 's/^/  /' "$ENV_FILE"
else
  printf '  (absent)\n'
fi

printf '\n== forward markers ==\n'
if [ -d "$FORWARDS" ]; then
  found=
  for marker in "$FORWARDS"/*; do
    [ -f "$marker" ] || continue
    found=yes
    while IFS= read -r line; do
      printf '  %s = %s\n' "$(basename "$marker")" "$line"
      break
    done < "$marker"
  done
  [ -n "$found" ] || printf '  (directory is empty)\n'
else
  printf '  (no forwards directory)\n'
fi

printf '\n== listeners ==\n'
if command -v ss >/dev/null; then
  ss -ltn | sed 's/^/  /'
else
  printf '  (ss unavailable)\n'
fi

printf '\n== reachability of discovered endpoints ==\n'
sources=""
[ -f "$ENV_FILE" ] && sources="$ENV_FILE"
if [ -d "$FORWARDS" ]; then
  for marker in "$FORWARDS"/*; do
    [ -f "$marker" ] && sources="$sources $marker"
  done
fi
if [ -z "$sources" ]; then
  printf '  (nothing declares an endpoint on this host)\n'
else
  # shellcheck disable=SC2086
  grep -ho 'http://[^"[:space:]]*' $sources | sort -u | while IFS= read -r url; do
    code=$(curl -s -o /dev/null -w '%{http_code}' "$url/healthz" || printf 'no-answer')
    printf '  %s/healthz -> %s\n' "$url" "$code"
  done
fi
