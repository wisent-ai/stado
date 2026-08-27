#!/bin/sh
# Recover the local credential path only when Skarbiec reports an audit-lock stall.
# Installed and invoked through `stado host install-helper` / `run-helper`.
set -eu

health_url="${SKARBIEC_HEALTH_URL:-http://127.0.0.1:8787/health}"
health=$(/usr/bin/curl --silent --show-error --max-time 10 "$health_url" || true)

case "$health" in
  *'"ok":true'*)
    printf '%s\n' 'skarbiec audit path is healthy; no recovery needed'
    exit 0
    ;;
  *'audit journal lock'*|*'audit.append.lock'*) ;;
  *)
    printf '%s\n' "refusing recovery: $health_url did not report an audit-lock failure" >&2
    exit 1
    ;;
esac

kick_loaded() {
  label=$1
  if /bin/launchctl print "gui/$(/usr/bin/id -u)/$label" >/dev/null 2>&1; then
    /bin/launchctl kickstart -k "gui/$(/usr/bin/id -u)/$label"
  fi
}

# Every restart is in-place. Dependencies return before their consumers move.
kick_loaded com.wisent.skarbiec
attempt=0
while [ "$attempt" -lt 30 ]; do
  health=$(/usr/bin/curl --silent --show-error --max-time 5 "$health_url" || true)
  case "$health" in
    *'"ok":true'*) break ;;
  esac
  attempt=$((attempt + 1))
  /bin/sleep 1
done
case "$health" in
  *'"ok":true'*) ;;
  *)
    printf '%s\n' "skarbiec stayed unhealthy after in-place recovery: $health" >&2
    exit 1
    ;;
esac

kick_loaded com.wisent.compute.service.skarbiec
kick_loaded com.wisent.compute.service.skarbiec-control-plane
kick_loaded com.wisent.compute.service.stado-object-api
printf '%s\n' 'skarbiec audit path recovered and dependent units restarted in place'
