#!/bin/sh
set -eu

source="$HOME/.stado/files/stado-agent-candidate"
target="$HOME/.stado/bin/stado"
next="$target.next"
[ -f "$source" ]
/bin/mkdir -p "$HOME/.stado/bin"
/usr/bin/install -m 0755 "$source" "$next"
/bin/mv "$next" "$target"
"$target" --version
