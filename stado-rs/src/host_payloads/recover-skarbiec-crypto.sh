#!/bin/sh
# Recover Skarbiec when its per-user GnuPG daemons have wedged the keybox or
# outgrown their memory ceiling. Invoked by the declared `skarbiec/crypto`
# repair step and by the host memory policy's `reap_recovery`.
#
# Two preconditions admit a recovery, and each is read from the host rather
# than assumed. Skarbiec's readiness reporting that gpg stopped answering, or
# a keybox lock, is the wedge. A keyboxd or gpg-agent of this account holding
# more than SKARBIEC_GPG_DAEMON_MEMORY_LIMIT_MB (Skarbiec's own ceiling; 1024
# unset) is the bloat: `gpg` never stops its daemons and keyboxd grows with
# every lookup, and on charless-mac-mini it held 15 GiB after twelve days
# while readiness answered ok, so the memory policy asked this program and it
# refused. The footprint read here is the physical one — resident, compressed
# and swapped pages together — because `ps` reported that daemon at 327 MiB.
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

health_url="${SKARBIEC_READY_URL:-http://127.0.0.1:8895/readyz}"
limit_mb="${SKARBIEC_GPG_DAEMON_MEMORY_LIMIT_MB:-1024}"

# The pid of one daemon serving this account's keyring, from GnuPG's own
# control surface with autostart off: a daemon that is not running answers no
# data line, and is not started to be measured. A host without GnuPG has no
# daemon to measure, and the wedge branch below refuses on gpgconf's absence.
daemon_pid() {
  command -v gpg-connect-agent >/dev/null 2>&1 || return 0
  case "$1" in
    keyboxd) gpg-connect-agent --no-autostart --keyboxd 'GETINFO pid' /bye 2>/dev/null ;;
    *) gpg-connect-agent --no-autostart 'GETINFO pid' /bye 2>/dev/null ;;
  esac | while IFS= read -r line; do
    case "$line" in
      'D '*) printf '%s\n' "${line#D }" ;;
    esac
  done || true
}

# Physical footprint of one pid in MiB. On macOS `top` reports the same MEM
# column Activity Monitor shows, with a unit suffix; on Linux /proc gives
# resident and swapped kilobytes.
footprint_mb() {
  pid=$1
  if [ -x /usr/bin/top ] && [ "$(uname -s)" = Darwin ]; then
    reading=$(/usr/bin/top -l 1 -pid "$pid" -stats pid,mem 2>/dev/null | tail -1)
    set -- $reading
    [ "${1:-}" = "$pid" ] || { printf '0\n'; return; }
    mem=${2:-0}
    mem=${mem%[+-]}
    case "$mem" in
      *G) printf '%s\n' $(( ${mem%G} * 1024 )) ;;
      *M) printf '%s\n' "${mem%M}" ;;
      *K) printf '%s\n' $(( ${mem%K} / 1024 )) ;;
      *) printf '0\n' ;;
    esac
    return
  fi
  if [ -r "/proc/$pid/status" ]; then
    rss_kb=$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")
    swap_kb=$(awk '/^VmSwap:/ {print $2}' "/proc/$pid/status")
    printf '%s\n' $(( (${rss_kb:-0} + ${swap_kb:-0}) / 1024 ))
    return
  fi
  printf '0\n'
}

# Every daemon of this account standing over the ceiling, one per line.
daemons_over_ceiling() {
  for name in keyboxd gpg-agent; do
    pid=$(daemon_pid "$name")
    [ -n "$pid" ] || continue
    mb=$(footprint_mb "$pid")
    if [ "$mb" -gt "$limit_mb" ]; then
      printf '%s pid %s holds %s MiB, over the %s MiB ceiling\n' "$name" "$pid" "$mb" "$limit_mb"
    fi
  done
}

set +e
health=$(/usr/bin/curl --silent --show-error --connect-timeout 2 --max-time 70 "$health_url")
health_status=$?
set -e
bloated=$(daemons_over_ceiling)
case "$health_status:$health" in
  0:*'"ok":true'*)
    if [ -z "$bloated" ]; then
      printf '%s\n' 'skarbiec cryptographic path is healthy and its GnuPG daemons are under their memory ceiling; no recovery needed'
      exit 0
    fi
    printf 'recovering: %s\n' "$bloated"
    ;;
  28:*|0:*'gpg'*'timed out'*|0:*'GPG'*'timed out'*|0:*'keybox'*'lock'*) ;;
  *)
    if [ -z "$bloated" ]; then
      printf '%s\n' "refusing recovery: Skarbiec did not report a GPG failure or keybox lock, and no GnuPG daemon of this account stands over the $limit_mb MiB ceiling" >&2
      exit 1
    fi
    printf 'recovering: %s\n' "$bloated"
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
while [ "$attempt" -lt 2 ]; do
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
