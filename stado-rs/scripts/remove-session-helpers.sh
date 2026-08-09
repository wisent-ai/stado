#!/bin/sh
# Take back every helper this investigation installed on a fleet host.
#
# `install-helper` leaves a file on someone else's machine; a diagnosis that
# does not remove its own tools leaves the next operator wondering what they
# are. Run from the control plane, not on the host.
#
# Usage: remove-session-helpers.sh <target>
set -eu

target=$1
stado=${STADO_BIN:-$HOME/.local/bin/stado}

for helper in \
    report-canonical-skarbiec \
    declare-canonical-skarbiec \
    provision-credential-lifecycle-grant \
    revoke-credential-lifecycle-grant \
    report-lifecycle-grants \
    report-azure-operator-item \
    report-directory-bindings \
    report-directory-seal-audit \
    report-vault-files \
    report-sealed-credentials \
    report-sealed-contract; do
    "$stado" host remove-helper "$target" "$helper" || printf 'could not remove %s\n' "$helper"
done
