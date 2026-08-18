#!/usr/bin/env bash
# Stop an unsupervised `stado release agent` that no launchd job owns.
#
# On the always-on Mac this process has run since 2026-08-14 with ppid 1, and no
# loaded job in `gui/<uid>` or `system` claims it: the unit that started it was
# unloaded and launchd adopted the child. It reconciles every fifteen seconds from
# the binary image it started with, so installing a fixed Stado changes nothing and
# its verdicts overwrite the release state file -- which is why a repaired agent kept
# producing the old failure wording.
#
# It serves no traffic. The release proxy is a separate process and keeps forwarding
# on the stable bind while this stops, which is the whole reason this is safe and a
# restart of the serving proxy would not be.
#
# Refuses anything that is not exactly that orphan: a supervised job must be
# restarted through its own unit, not killed here. Idempotent.
set -euo pipefail

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
/bin/ps -eo pid=,ppid=,command= > "$work/ps" 2>/dev/null || true
/usr/bin/grep 'stado release agent' "$work/ps" > "$work/agents" 2>/dev/null || true

count=$(/usr/bin/wc -l < "$work/agents" | /usr/bin/tr -d ' ')
printf 'agent_processes %s\n' "$count"
if [ "$count" = "0" ]; then
  printf 'nothing to stop\n'
  exit 0
fi

stopped=0
while IFS= read -r line; do
  pid=$(printf '%s\n' "$line" | /usr/bin/awk '{print $1}')
  ppid=$(printf '%s\n' "$line" | /usr/bin/awk '{print $2}')
  printf 'candidate pid=%s ppid=%s\n' "$pid" "$ppid"
  if [ "$ppid" != "1" ]; then
    printf '  supervised by %s; leaving it alone\n' "$ppid"
    continue
  fi
  /bin/kill -TERM "$pid" 2>/dev/null || {
    printf '  could not signal %s\n' "$pid" >&2
    continue
  }
  sleep 2
  if /bin/kill -0 "$pid" 2>/dev/null; then
    printf '  pid %s still alive after SIGTERM; not escalating\n' "$pid" >&2
  else
    printf '  stopped %s\n' "$pid"
    stopped=$((stopped + 1))
  fi
done < "$work/agents"

printf 'stopped_total %s\n' "$stopped"

# The bind must still serve: this process was never in the request path, and saying
# so with a status is cheaper than trusting the claim.
code=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' -m 5 "http://127.0.0.1:8080/health" 2>/dev/null || echo 000)
printf 'stable_bind_http %s\n' "$code"
