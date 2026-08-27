#!/bin/sh
# Recover the credential path when Skarbiec stalls or Stado retains a closed
# object boundary after Skarbiec has recovered. Invoked by
# `stado host recover-skarbiec-audit`.
set -eu

health_url="${SKARBIEC_HEALTH_URL:-http://127.0.0.1:8787/health}"
object_health_url="${STADO_OBJECT_HEALTH_URL:-http://127.0.0.1:18765/healthz}"
health=$(/usr/bin/curl --silent --show-error --max-time 10 "$health_url" || true)
object_health=$(/usr/bin/curl --silent --show-error --max-time 10 "$object_health_url" || true)

kick_loaded() {
  label=$1
  if /bin/launchctl print "gui/$(/usr/bin/id -u)/$label" >/dev/null 2>&1; then
    /bin/launchctl kickstart -k "gui/$(/usr/bin/id -u)/$label"
  fi
}

case "$health" in
  *'"ok":true'*) audit_recovered=false ;;
  *'audit journal lock'*|*'audit.append.lock'*)
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
      *'"ok":true'*) audit_recovered=true ;;
      *)
        printf '%s\n' "skarbiec stayed unhealthy after in-place recovery: $health" >&2
        exit 1
        ;;
    esac
    kick_loaded com.wisent.compute.service.skarbiec
    kick_loaded com.wisent.compute.service.skarbiec-control-plane
    ;;
  *)
    printf '%s\n' "refusing recovery: $health_url did not report an audit-lock failure" >&2
    exit 1
    ;;
esac

case "$object_health" in
  *'"object":true'*)
    if [ "$audit_recovered" = false ]; then
      printf '%s\n' 'skarbiec and Stado object authorization are healthy; no recovery needed'
      exit 0
    fi
    ;;
  *'"object":false'*)
    kick_loaded com.wisent.compute.service.stado-object-api
    ;;
  *)
    printf '%s\n' "refusing recovery: $object_health_url returned no object boundary verdict" >&2
    exit 1
    ;;
esac

attempt=0
while [ "$attempt" -lt 30 ]; do
  object_health=$(/usr/bin/curl --silent --show-error --max-time 5 "$object_health_url" || true)
  case "$object_health" in
    *'"object":true'*) break ;;
  esac
  attempt=$((attempt + 1))
  /bin/sleep 1
done
case "$object_health" in
  *'"object":true'*) ;;
  *)
    printf '%s\n' "Stado object authorization stayed closed after recovery: $object_health" >&2
    exit 1
    ;;
esac
printf '%s\n' 'skarbiec audit path and Stado object authorization recovered'
