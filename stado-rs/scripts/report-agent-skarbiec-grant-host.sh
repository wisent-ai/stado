#!/bin/sh
# Which Skarbiec grant file this host's agent is configured to use, and whether
# it exists.
#
# The agent logs `[vast] cannot read stado-vast/api_key from Skarbiec: cannot
# read Skarbiec grant file /root/.stado/control-plane-skarbiec-token: No such
# file or directory`, and that gate is what pauses fleet claims while a Vast
# renter is on the card. Before minting anything, the cheap question is whether
# the configured path is simply the wrong name for a grant this host already
# holds.
#
# Read-only, and no token value is ever printed: names, sizes and existence only.
set -eu

printf 'CONFIGURED\n'
systemctl show wisent-agent.service --property=Environment --value |
  tr ' ' '\n' |
  grep -Ei 'skarbiec|grant' || printf 'nothing in the unit environment\n'

for file in /root/.stado/files/stado-agent-grant.env /root/.stado/stado-agent-grant.env /root/.stado/stado-agent.env; do
  if [ -r "$file" ]; then
    printf '\nFILE\t%s\n' "$file"
    grep -Ei '^[A-Z_]*(SKARBIEC|GRANT)[A-Z_]*=' "$file" |
      while IFS='=' read -r name value; do
        case "$name" in
          *TOKEN|*SECRET|*KEY|*BEARER)
            if [ -n "$value" ]; then printf '%s\t(value withheld)\n' "$name"; else printf '%s\tempty\n' "$name"; fi
            ;;
          *) printf '%s\t%s\n' "$name" "$value" ;;
        esac
      done
  fi
done

printf '\nGRANT_FILES_PRESENT\n'
for candidate in /root/.stado/*skarbiec-token* /root/.stado/*grant*; do
  [ -e "$candidate" ] || continue
  printf '%s\t%s bytes\t%s\n' "$candidate" "$(stat -c %s "$candidate")" "$(stat -c %y "$candidate" | cut -d. -f1)"
done
