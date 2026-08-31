#!/bin/sh
# Recover Skarbiec when stale per-user GnuPG daemons hold the keybox lock.
# Invoked by `stado host recover-skarbiec-crypto`.
set -eu

health_url="${SKARBIEC_READY_URL:-http://127.0.0.1:8895/readyz}"
health=$(/usr/bin/curl --silent --show-error --max-time 70 "$health_url" || true)
case "$health" in
  *'"ok":true'*)
    printf '%s\n' 'skarbiec cryptographic path is healthy; no recovery needed'
    exit 0
    ;;
  *'gpg'*'timed out'*|*'GPG'*'timed out'*|*'keybox'*'lock'*) ;;
  *)
    printf '%s\n' 'refusing recovery: Skarbiec did not report a GPG timeout or keybox lock' >&2
    exit 1
    ;;
esac

uid=$(/usr/bin/id -u)
gpgconf=$(command -v gpgconf || true)
if [ -z "$gpgconf" ]; then
  printf '%s\n' 'refusing recovery: gpgconf is not installed' >&2
  exit 1
fi

stop_owned() {
  signal=$1
  name=$2
  /usr/bin/sudo -n /usr/bin/pkill "-$signal" -U "$uid" -x "$name" >/dev/null 2>&1 || true
}

keybox_pids=
keybox_db="${GNUPGHOME:-$HOME/.gnupg}/public-keys.d/pubring.db"
if [ -f "$keybox_db" ] && [ -x /usr/sbin/lsof ]; then
  for pid in $(/usr/bin/sudo -n /usr/sbin/lsof -t "$keybox_db" 2>/dev/null || true); do
    comm=$(/bin/ps -p "$pid" -o comm= 2>/dev/null || true)
    case "$comm" in
      *keyboxd)
        keybox_pids="$keybox_pids $pid"
        /usr/bin/sudo -n /bin/kill -TERM "$pid"
        ;;
    esac
  done
fi

for name in gpg keyboxd gpg-agent; do
  stop_owned TERM "$name"
done
/bin/sleep 2
for pid in $keybox_pids; do
  comm=$(/bin/ps -p "$pid" -o comm= 2>/dev/null || true)
  case "$comm" in
    *keyboxd) /usr/bin/sudo -n /bin/kill -KILL "$pid" ;;
  esac
done
for name in gpg keyboxd gpg-agent; do
  stop_owned KILL "$name"
done

"$gpgconf" --launch keyboxd
"$gpgconf" --launch gpg-agent

attempt=0
while [ "$attempt" -lt 3 ]; do
  health=$(/usr/bin/curl --silent --show-error --max-time 70 "$health_url" || true)
  case "$health" in
    *'"ok":true'*)
      printf '%s\n' 'skarbiec cryptographic daemons recovered'
      exit 0
      ;;
  esac
  attempt=$((attempt + 1))
  /bin/sleep 2
done

printf '%s\n' 'Skarbiec stayed unhealthy after GPG daemon recovery' >&2
exit 1
