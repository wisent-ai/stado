#!/bin/sh
# Why this host's agent is, or is not, broadcasting capacity.
#
# The fleet places GPU work out of `capacity/<id>.json` objects that each agent
# publishes; a host with no such object is a host the scheduler cannot size or
# place a job on, however healthy its unit looks. On 2026-08-17 the only GPU
# host in the registry had `wisent-agent.service` active and no capacity object
# at all, and `stado host health` shows unit state without showing that.
#
# Read-only: unit state, the agent's own recent log, and the storage-related
# variables its unit carries (names and whether a value is present -- never the
# value, because these units carry bearer tokens).
set -eu

unit=wisent-agent.service

printf 'UNIT_STATE\t'
systemctl is-active "$unit" 2>&1 || true
printf 'UNIT_SINCE\t'
systemctl show "$unit" --property=ActiveEnterTimestamp --value 2>&1 || true
printf 'UNIT_EXEC\t'
systemctl show "$unit" --property=ExecStart --value 2>&1 | head -n 1 || true
printf 'RESTARTS\t'
systemctl show "$unit" --property=NRestarts --value 2>&1 || true

printf '\nSTORAGE_ENV\n'
systemctl show "$unit" --property=Environment --value 2>&1 |
  tr ' ' '\n' |
  while IFS= read -r pair; do
    case "$pair" in
      WC_*|STADO_*|WISENT_*)
        name=${pair%%=*}
        value=${pair#*=}
        case "$name" in
          *TOKEN*|*SECRET*|*KEY*|*PASSWORD*)
            if [ -n "$value" ]; then printf '%s\tpresent\n' "$name"; else printf '%s\tempty\n' "$name"; fi
            ;;
          *) printf '%s\t%s\n' "$name" "$value" ;;
        esac
        ;;
    esac
  done

printf '\nCONFIG_STORAGE\n'
for candidate in /root/.config/stado/config.json "$HOME/.config/stado/config.json"; do
  if [ -r "$candidate" ]; then
    printf '%s\n' "$candidate"
    sed -n 's/.*"\(backend\|url\|namespace\|token_file\)"[[:space:]]*:[[:space:]]*\("[^"]*"\|[0-9]*\).*/  \1 \2/p' "$candidate"
  fi
done

printf '\nAGENT_LOG_TAIL\n'
journalctl -u "$unit" --no-pager -n 40 -o cat 2>&1 | tail -n 40 || true
