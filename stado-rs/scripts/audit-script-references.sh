#!/bin/sh
# Which operator scripts does nothing else mention?
#
# A diagnostic written during one investigation is a second source of truth the
# next reader has to rule out. This prints, for each named script, every file
# that references it apart from the script itself, so an unreferenced one can
# be removed with a positive reason rather than a hunch.
#
# Usage: audit-script-references.sh <script-basename> [<script-basename> ...]
set -eu

root=$(/usr/bin/dirname "$0")/../..

for name in "$@"; do
    hits=$(/usr/bin/grep -rl "$name" \
        --include='*.md' --include='*.rs' --include='*.sh' --include='*.py' \
        --include='*.yml' --include='*.json' "$root" |
        /usr/bin/grep -v "scripts/$name" |
        /usr/bin/tr '\n' ' ')
    printf '%-36s %s\n' "$name" "${hits:-(unreferenced)}"
done
