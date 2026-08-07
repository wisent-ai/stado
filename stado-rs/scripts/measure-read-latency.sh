#!/bin/sh
# How long does one authorized field read cost end to end?
#
# `stado doctor` went from six seconds to fifty without a code change in the
# sweep, so the question is whether the listener itself got slower. Ten reads
# of one field, wall time around the loop; no value is printed.
#
# Usage: measure-read-latency.sh <item> <field>
set -eu

config=${STADO_CONFIG:-$HOME/.config/stado/config.json}
item=$1
field=$2
url=$(/usr/bin/jq -r '.secrets.skarbiec.url' "$config")
token=$(/bin/cat "$HOME/.stado/local-operator-skarbiec-token")

started=$(/bin/date +%s)
for _ in a b c d e f g h i j; do
    /usr/bin/curl -sS -o /dev/null \
        -X POST "$url/v1/items/read" \
        -H "Authorization: Bearer $token" \
        -H "X-Consumer: local-operator" \
        -H 'Content-Type: application/json' \
        --data "{\"id\":\"$item\",\"field\":\"$field\"}"
done
finished=$(/bin/date +%s)
printf 'ten reads of %s#%s took %s s\n' "$item" "$field" "$((finished - started))"
