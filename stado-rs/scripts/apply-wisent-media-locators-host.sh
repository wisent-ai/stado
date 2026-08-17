#!/bin/sh
# Apply the media locator rewrite planned by `rewrite-wisent-media-locators`.
#
# Two files rather than one flag: `stado host run-helper` deliberately passes no
# operator arguments, so "report" and "write to the live product database" must
# be two separately named, separately installed helpers. Installing this one is
# the authorization; running it performs the update and re-reads every row it
# touched.
#
# The reporting helper writes the before-image first, and this wrapper reuses
# exactly its logic, so the reversal file always matches what was applied.
set -eu

helper=${WISENT_REWRITE_HELPER:-$HOME/.stado/bin/rewrite-wisent-media-locators}
[ -x "$helper" ] || {
    printf '%s\n' "install rewrite-wisent-media-locators first: $helper" >&2
    exit 1
}
MODE=apply exec "$helper"
