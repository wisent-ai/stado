#!/bin/sh
# Every Skarbiec vault and audit file this host carries, with size and date.
#
# "The fleet vault" is not a path anyone wrote down, and looking in the two
# obvious files answered nothing. This lists what actually exists, so the next
# question is asked of the right file instead of the expected one.
set -eu

printf '== vault and audit files ==\n'
/usr/bin/find "$HOME/.stado" -maxdepth 2 \( -name '*.vault.json' -o -name '*.audit.json*' \) \
    -exec /bin/ls -l {} + | /usr/bin/awk '{print $5, $6, $7, $8, $9}'

printf '== items per vault ==\n'
for vault in $(/usr/bin/find "$HOME/.stado" -maxdepth 2 -name '*.vault.json'); do
    printf '%s\t%s items\n' "$vault" "$(/usr/bin/jq -r '(.items // {}) | length' "$vault")"
done
