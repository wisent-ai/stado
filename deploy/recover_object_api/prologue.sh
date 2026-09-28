#!/usr/bin/env bash
# Restore the Stado object API without trusting a successful read alone.
#
# launchd's loaded job and its plist are separate facts. Recovery proves the
# loaded storage route and authenticated reads before reporting readiness.
# A different authority requires `host storage-root-reconcile`, which owns the
# snapshots, writer fence, namespace-qualified copy, and rollback. This helper
# repairs only the same-root listener using the host's canonical delivered Stado.
set -euo pipefail

program="$HOME/.stado/bin/stado"
config="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
work="$HOME/.stado/work/object-api-recovery"

if [ "$(/usr/bin/uname -s)" != "Darwin" ]; then
  printf 'unsupported_os %s\n' "$(/usr/bin/uname -s)" >&2
  exit 65
fi
if [ ! -x "$program" ]; then
  printf 'program_missing %s\n' "$program" >&2
  exit 66
fi

# Where the stores are, how the object route is addressed, and which launchd
# label the host Stado process runs under: the environment first for the
# stores, then the host config, then the managed defaults, and the label from
# the catalog compiled into the host's own Stado. This program only carries
# the answer.
coordinates=$("$program" host object-api-local paths --config "$config")
IFS=$'\t' read -r store backup_store object_url object_namespace object_token_file label \
  <<< "$coordinates"
if [ -z "$label" ]; then
  printf 'host_stado_names_no_unit %s: deliver the current Stado to this host first\n' \
    "$program" >&2
  exit 71
fi
plist="/Library/LaunchDaemons/$label.plist"
log="$HOME/.stado/logs/$label.log"
if [ ! -d "$store" ] || [ ! -r "$store/registry.json" ]; then
  printf 'local_store_missing %s\n' "$store" >&2
  exit 67
fi
if [ "$backup_store" = "$store" ]; then
  printf 'local_backup_matches_primary %s\n' "$store" >&2
  exit 68
fi
/bin/mkdir -p "$backup_store"
/bin/chmod 700 "$backup_store"

/bin/mkdir -p "$work" "$HOME/.stado/logs"
/bin/chmod 700 "$work" "$HOME/.stado/logs"
/usr/bin/touch "$log"
/bin/chmod 600 "$log"
staged=$(/usr/bin/mktemp "$work/$label.plist.XXXXXX")
trap '/bin/rm -f "$staged"' EXIT HUP INT TERM
account=$(/usr/bin/id -un)

# Recovery owns the executable and required environment, not every launchd
# option: the definition keeps fields the shared service renderer installed,
# resource limits included, instead of rewriting a healthy unit.
"$program" host object-api-local render-plist --staged "$staged" --installed "$plist" \
  --label "$label" --program "$program" --store "$store" --backup-store "$backup_store" \
  --account "$account" --log "$log" --config "$config"
/usr/bin/plutil -lint "$staged" >/dev/null



