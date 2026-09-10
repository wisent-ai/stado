#!/usr/bin/env bash
# Report which provider resources the gateway can turn into a vault coordinate.
#
# Brama reads a routes table named by SKARBIEC_CAPABILITY_ROUTES_FILE and skips
# any subscription whose resource is absent from it, logging one warning per
# provider at startup and then serving as if that provider did not exist. A
# caller asking for a capability alias -- "any-vision-capable" -- sees only a
# 429 with no working model, which names neither the provider nor the missing
# row.
#
# Read-only, and prints names only: route keys, their vault item and field, and
# the vault items whose names mention the skipped providers. No secret value is
# read or printed.
set -u

routes="$HOME/.stado/capability-routes.json"
vault="$HOME/.stado/skarbiec.vault.json"
jq_bin=$(command -v jq || true)

echo "=== routes table ==="
echo "file: $routes"
if [ ! -r "$routes" ]; then
  echo "state: absent or unreadable"
elif [ -n "$jq_bin" ]; then
  "$jq_bin" -r 'to_entries[] | "\(.key) -> \(.value.item)#\(.value.field)"' "$routes" 2>/dev/null | sort
else
  /usr/bin/sed -n '/"/p' "$routes" | head -40
fi

echo
echo "=== subscription providers the gateway tried to load ==="
/usr/bin/sed -n 's/.*skipping subscription //p' "$HOME/.stado/logs/brama-always-on.err" 2>/dev/null | sort -u | head -8
echo "(empty means this start logged no skipped subscription)"

echo
echo "=== vault items whose names mention those providers ==="
if [ -r "$vault" ] && [ -n "$jq_bin" ]; then
  "$jq_bin" -r '[.items[]? | select(.deleted != true) | .id] | .[]' "$vault" 2>/dev/null \
    | /usr/bin/grep -Ei 'claude|codex|kimi|vision|anthropic|openai' | sort | head -20
else
  echo "(vault unreadable here, or jq is absent)"
fi
