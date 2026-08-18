#!/usr/bin/env bash
# Report whether a deployed release's launcher carries an expected marker.
#
# A candidate kept failing the same policy check after the launcher was fixed, and
# there are three ways that can be true at once: the archive does not contain the
# fix, the launcher resolves a different file than expected, or the file it
# resolves really lacks the key. This distinguishes the first from the rest by
# reading the launcher that was actually deployed, rather than the one in the
# repository.
#
# Read-only: greps for markers and prints the resolved config path, never a secret.
set -euo pipefail

product="${RELEASE_PRODUCT:-brama}"
version="${RELEASE_VERSION:-}"
root="${RELEASE_SERVICES_ROOT:-$HOME/.stado/services/$product/releases}"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'releases_root %s\n' "$root"
[ -d "$root" ] || { printf 'releases_root absent\n'; exit 0; }

if [ -z "$version" ]; then
  version=$(/bin/ls -t "$root" 2>/dev/null | /usr/bin/head -1)
fi
printf 'version %s\n' "${version:-none}"
[ -n "$version" ] || exit 0

launcher=$(/usr/bin/find "$root/$version" -type f -name 'start-with-skarbiec' 2>/dev/null | /usr/bin/head -1)
if [ -z "$launcher" ]; then
  printf 'launcher absent under %s\n' "$root/$version"
  exit 0
fi
printf 'launcher %s\n' "$launcher"
printf 'bytes %s\n' "$(/usr/bin/wc -c <"$launcher" | /usr/bin/tr -d ' ')"

for marker in brama_service_env BRAMA_SERVICE_ENV 'exact closed Brama alias set'; do
  if /usr/bin/grep -q "$marker" "$launcher"; then
    printf 'marker present %s\n' "$marker"
  else
    printf 'marker ABSENT %s\n' "$marker"
  fi
done

# What that launcher would resolve, evaluated the same way it does.
service_env="${BRAMA_SERVICE_ENV:-$HOME/.config/brama/service.env}"
resolved=""
if [ -f "$service_env" ]; then
  resolved=$(/usr/bin/sed -n 's/^[[:space:]]*BRAMA_CONTROL_CONFIG[[:space:]]*=[[:space:]]*//p' "$service_env" \
    | /usr/bin/tail -1 | /usr/bin/tr -d "\"'")
fi
printf 'service_env %s\n' "$service_env"
printf 'would_resolve %s\n' "${resolved:-$HOME/.config/brama/control.json}"
