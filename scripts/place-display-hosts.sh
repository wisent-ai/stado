#!/bin/sh
# Ask the placement question the browser logins ask, from a fleet host.
#
# `stado host run-helper` deliberately hands a helper no operator words -- a
# helper that took them would be a remote shell -- so the question a host may be
# asked is checked in rather than typed. This is that question: which host can
# own a window and render a page right now, measured no longer ago than the
# five minutes a login session is worth trusting for.
#
# Exits 0 printing one host name, or non-zero printing one line per candidate
# with the measurement that disqualified it.
set -eu
exec "$HOME/.stado/bin/place-by-capability" --requires display,browser-render --max-stale-seconds 300
