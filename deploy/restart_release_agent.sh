#!/usr/bin/env bash
# Restart this host's resident release agent in place, so it loads the installed Stado.
#
# `stado release agent --interval-seconds 15` has been running since 2026-08-14 from
# the binary image it started with. Installing a new Stado therefore changes nothing
# about its verdicts, and it rewrites the release state every fifteen seconds, which
# is why a fixed agent kept producing the old failure wording.
#
# It is not a registry-managed service, so `stado service restart` refuses it. This
# uses `launchctl kickstart -k`, which restarts a loaded job without a window in
# which it does not exist -- an unload/load pair can leave nothing running, and that
# is how a Weles worker was lost earlier this month.
#
# The agent serves no traffic: the release proxy is a separate process and keeps
# forwarding while this restarts. Undo is the same command.
set -euo pipefail

label="${RELEASE_AGENT_LABEL:-com.wisent.compute.service.stado-agent-mini}"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'label %s\n' "$label"

# `awk ... exit` closes the pipe while `ps` is still writing, so under `pipefail`
# the whole pipeline failed and this script exited right after printing the label.
before=$(/bin/ps -eo pid=,command= 2>/dev/null | /usr/bin/grep 'stado release agent' \
  | /usr/bin/head -1 | /usr/bin/awk '{print $1}' || true)
printf 'before_pid %s\n' "${before:-none}"

domain="gui/$(id -u)"
if /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
  target="$domain/$label"
elif /bin/launchctl print "system/$label" >/dev/null 2>&1; then
  target="system/$label"
else
  printf 'label not loaded in gui or system domain; nothing restarted\n' >&2
  exit 66
fi
printf 'domain %s\n' "$target"

/bin/launchctl kickstart -k "$target"
sleep 3

after=$(/bin/ps -eo pid=,command= 2>/dev/null | /usr/bin/grep 'stado release agent' \
  | /usr/bin/head -1 | /usr/bin/awk '{print $1}' || true)
printf 'after_pid %s\n' "${after:-none}"
if [ -z "$after" ]; then
  printf 'agent did not come back\n' >&2
  exit 67
fi
if [ "$after" = "${before:-}" ]; then
  printf 'pid unchanged; the job may not have restarted\n' >&2
  exit 68
fi
printf 'restarted\n'
