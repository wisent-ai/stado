#!/usr/bin/env bash
# Report which Stado binary this host runs, and whether it carries given markers.
#
# A fix was installed and the host's recorded failure kept the old wording, which
# has two explanations: the record is stale, or the running binary is not the one
# installed. Reading strings out of the binary settles it without guessing, and
# `install-binary` writes to `~/.stado/bin/stado` while a launchd job may execute a
# different copy.
#
# Read-only: paths, sizes, timestamps and marker presence. No secret is read.
set -euo pipefail

primary="$HOME/.stado/bin/stado"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
for candidate in "$primary" /usr/local/bin/stado /opt/homebrew/bin/stado; do
  if [ -x "$candidate" ]; then
    printf 'binary %s %s bytes %s\n' "$candidate" \
      "$(/usr/bin/wc -c <"$candidate" | /usr/bin/tr -d ' ')" \
      "$(/usr/bin/stat -f '%Sm' -t '%Y-%m-%dT%H:%M:%SZ' "$candidate" 2>/dev/null || true)"
  else
    printf 'binary %s absent\n' "$candidate"
  fi
done

[ -x "$primary" ] || exit 0
for marker in 'adopted the proxy already serving' 'candidate did not become ready within' 'stable release proxy failed to start'; do
  if /usr/bin/grep -qa "$marker" "$primary"; then
    printf 'marker present %s\n' "$marker"
  else
    printf 'marker ABSENT %s\n' "$marker"
  fi
done

# Which copy the release agent would actually execute for this host.
printf -- '--- agent processes ---\n'
/bin/ps -eo pid=,command= 2>/dev/null \
  | /usr/bin/awk '/stado (release|host) /{print "  " $0}' | /usr/bin/cut -c1-140 | /usr/bin/head -5
