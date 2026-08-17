#!/bin/sh
# Apply the revert planned by `revert-wisent-media-locators`.
#
# Separate file for the same reason the forward pass has one: `stado host
# run-helper` passes no operator arguments, so "report" and "write to the live
# product database" must be two separately installed helpers, and installing
# this one is the authorization.
set -eu

helper=${WISENT_REVERT_HELPER:-$HOME/.stado/bin/revert-wisent-media-locators}
[ -x "$helper" ] || {
    printf '%s\n' "install revert-wisent-media-locators first: $helper" >&2
    exit 1
}
MODE=apply exec "$helper"
