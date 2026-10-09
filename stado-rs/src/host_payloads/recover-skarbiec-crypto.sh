#!/bin/sh
# Recover Skarbiec when its per-user GnuPG daemons have wedged the keybox,
# a process keeps the keyring's lock, or a daemon has outgrown its memory
# ceiling. Invoked by the declared `skarbiec/crypto` repair step and by the
# host beacon role when it reads one of those grounds in the host's own
# Skarbiec readiness.
#
# Three preconditions admit a recovery, and each is read from the host rather
# than assumed. Skarbiec's readiness reporting that gpg stopped answering, or
# a keybox lock, is the wedge; readiness naming the keyring lock's holder
# (`waiting for lock (held by <pid>)`, `<lock> is held by pid <pid>`) or not
# answering within the probe's own limit is the held lock. A keyboxd or
# gpg-agent of this account holding more than
# SKARBIEC_GPG_DAEMON_MEMORY_LIMIT_MB (Skarbiec's own ceiling; 1024 unset) is
# the bloat: `gpg` never stops its daemons and keyboxd grows with every
# lookup, to many GiB over days while readiness answers ok. The footprint
# read here is the physical one — resident, compressed and swapped pages
# together — because `ps` reports such a daemon at a fraction of it.
#
# The repair itself is Skarbiec's own: `skarbiec recover-daemons` replaces
# the account's keyboxd, gpg-agent and scdaemon through gpgconf and releases
# a keyring lock whose holder can no longer let go — a dead or reused pid, a
# lock written under another host name, a GnuPG program that held it past
# gpg's own wait — the same repair the vault runs for itself after a failed
# read, run here for the case where the vault's own run never reached it.
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

# Readiness comes from the caller (SKARBIEC_READY_URL) or this host's
# skarbiec forward marker; no port is written here.
if [ -z "${SKARBIEC_READY_URL:-}" ]; then
  marker="$HOME/.stado/forwards/skarbiec.local"
  [ -s "$marker" ] || { printf '%s\n' "no skarbiec address: $marker is missing or empty" >&2; exit 2; }
  SKARBIEC_READY_URL="$(tr -d '[:space:]' < "$marker")/readyz"
fi
health_url="$SKARBIEC_READY_URL"
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
      exit 3
    fi
    printf 'recovering: %s\n' "$bloated"
    ;;
  # The lock gpg waits on, named by gpg itself or by Skarbiec's lock-holder
  # line, whatever status the probe ended with.
  *'waiting for lock (held by '*|*'is held by pid '*)
    printf 'recovering: Skarbiec readiness names a held keyring lock: %s\n' "$health"
    ;;
  28:*|0:*'gpg'*'timed out'*|0:*'GPG'*'timed out'*|0:*'keybox'*'lock'*) ;;
  *)
    if [ -z "$bloated" ]; then
      printf '%s\n' "refusing recovery: Skarbiec did not report a GPG failure, a keybox lock or a held keyring lock, and no GnuPG daemon of this account stands over the $limit_mb MiB ceiling; readiness answered (curl status $health_status): $health" >&2
      exit 1
    fi
    printf 'recovering: %s\n' "$bloated"
    ;;
esac

skarbiec="$HOME/.stado/bin/skarbiec"
[ -x "$skarbiec" ] || printf '%s\n' "no Skarbiec binary at $skarbiec; install the skarbiec release on this host" >&2
# Skarbiec's own repair, against the keyring gpgconf names for this account
# (GNUPGHOME when the login shell carries it): what it signalled, what each
# daemon held and which lock it released are its own lines, kept in this
# payload's output. It needs no vault and no unlock material.
"$skarbiec" recover-daemons || {
  status=$?
  printf '%s\n' "skarbiec recover-daemons exited $status on this host; its own lines above say which daemon or lock it could not act on" >&2
  exit "$status"
}

if ! health=$(/usr/bin/curl --silent --show-error "$health_url" 2>&1); then
  printf '%s\n' "Skarbiec health could not be read after GPG daemon recovery: $health" >&2
  exit 1
fi
case "$health" in
  *'"ok":true'*)
    printf '%s\n' 'skarbiec cryptographic daemons recovered'
    exit 0
    ;;
esac
printf '%s\n' "Skarbiec stayed unhealthy after GPG daemon recovery: $health" >&2
exit 1
