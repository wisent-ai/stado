#!/bin/sh
# Run one release build request on this host and show why it fails.
#
# The queue agent runs `stado release worker --request release-request.json`,
# and when that exits non-zero the job record keeps only "workload exited
# unsuccessfully; inspect the redacted command output" -- with no output object
# uploaded, because the worker fails before it produces one. Nothing in the
# fleet store says more, so the build has to be run where its stderr is visible.
#
# Read-only with respect to the queue: this stages the same request into a fresh
# directory and runs it there. It claims nothing and completes no job.
set -u


BIN="$HOME/.stado/bin/stado"
PLATFORM=darwin-arm64
work=$(/usr/bin/mktemp -d "$HOME/.stado/build-work/reproduce.XXXXXX")

# `host run-helper` hands a helper no operator words on purpose, so the run is
# discovered here: the newest Brama pipeline request published for this platform.
uri=$("$BIN" storage ls runs/release-pipeline/brama/ 2>/dev/null \
  | /usr/bin/awk -v p="requests/$PLATFORM.json" '$1 ~ p {print $1, $2}' \
  | /usr/bin/sort -k2 -r | /usr/bin/head -1 | /usr/bin/awk '{print $1}')
[ -n "$uri" ] || { printf 'no brama release request found\n' >/dev/stderr; /bin/rm -rf "$work"; exit 1; }
printf 'request=%s\n' "$uri"

"$BIN" storage get "stado://probierz/$uri" "$work/release-request.json" >/dev/null 2>&1 || {
  printf 'could not fetch %s\n' "$uri" >/dev/stderr
  /bin/rm -rf "$work"
  exit 1
}
cd "$work" || exit 1
"$BIN" release worker --request release-request.json >"$work/out.log" 2>&1
rc=$?
printf 'worker_rc=%s\n' "$rc"
printf '== last lines ==\n'
/usr/bin/tail -30 "$work/out.log" | /usr/bin/cut -c1-200
printf 'workdir=%s\n' "$work"
