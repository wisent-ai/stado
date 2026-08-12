#!/bin/sh
# Report which of this host's declared service endpoints actually answer.
#
# Install:
#   stado host install-helper <target> \
#     stado-rs/scripts/probe-service-endpoints.sh probe-service-endpoints
#
# `stado service verify` runs this on every host that holds a declaration and
# merges the answers into one table. It takes no arguments on purpose: the
# fleet channel restricts helper argv to correlation identifiers, and a helper
# that accepted a URL would be a remote fetcher with the audit trail removed.
# Everything it probes comes from the registry this host already resolves.
#
# A host without this helper is reported `unverified`, never `observed` and
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
