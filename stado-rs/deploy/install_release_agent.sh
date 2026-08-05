#!/bin/sh
set -eu
umask 022

if [ "$#" -ne 4 ]; then
  printf '%s\n' "usage: install_release_agent.sh <target> <home> <stado-bin> <stado-config>" >&2
  exit 2
fi

target=$1
home=$2
stado_bin=$3
stado_config=$4
case "$target" in *[!A-Za-z0-9._-]*|'') printf '%s\n' "target must be an exact registry name" >&2; exit 2;; esac
for value in "$home" "$stado_bin" "$stado_config"; do
  case "$value" in /*) ;; *) printf '%s\n' "paths must be absolute" >&2; exit 2;; esac
done
[ -x "$stado_bin" ] || { printf '%s\n' "Stado binary is not executable: $stado_bin" >&2; exit 1; }
[ -f "$stado_config" ] || { printf '%s\n' "Stado config is not a regular file: $stado_config" >&2; exit 1; }

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
template="$script_dir/com.wisent.stado.release-agent.plist.tmpl"
[ -f "$template" ] || { printf '%s\n' "missing release agent template: $template" >&2; exit 1; }

mkdir -p "$home/.stado/logs"
staging=$(mktemp /tmp/com.wisent.stado.release-agent.XXXXXX)
trap 'rm -f "$staging"' EXIT HUP INT TERM
escape_sed() { printf '%s' "$1" | sed 's/[|&\\]/\\&/g'; }
sed \
  -e "s|__TARGET__|$(escape_sed "$target")|g" \
  -e "s|__HOME__|$(escape_sed "$home")|g" \
  -e "s|__STADO_BIN__|$(escape_sed "$stado_bin")|g" \
  -e "s|__STADO_CONFIG__|$(escape_sed "$stado_config")|g" \
  "$template" > "$staging"
/usr/bin/plutil -lint "$staging" >/dev/null
/usr/bin/install -o root -g wheel -m 0644 "$staging" /Library/LaunchDaemons/com.wisent.stado.release-agent.plist
/bin/launchctl bootout system/com.wisent.stado.release-agent >/dev/null 2>&1 || true
/bin/launchctl bootstrap system /Library/LaunchDaemons/com.wisent.stado.release-agent.plist
printf '%s\n' "installed Stado release agent for $target"
