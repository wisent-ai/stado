#!/bin/sh
# Report which of this host's declared service endpoints actually answer.
#
# This script is embedded in the stado binary itself
# (`service_verify::PROBE_SCRIPT`, via include_str!). `stado service verify`
# runs it as one fixed remote script on every host that holds a declaration and
# merges the answers into one table. It takes no arguments on purpose: a probe
# that accepted a URL would be a remote fetcher with the audit trail removed.
# Everything it probes comes from the registry this host already resolves.
#
# A host whose probe cannot run is reported `unverified`, never `observed` and
# never `unreachable`. That is the whole point of the third state.
set -eu

stado="$HOME/.stado/bin/stado"
if [ ! -x "$stado" ]; then
  printf '%s\n' "missing executable Stado binary: $stado" >&2
  exit 69
fi

# --local exits non-zero when a declaration is unreachable, which is the answer
# the sweep wants recorded rather than treated as a broken probe.
"$stado" service verify --local --json || true
