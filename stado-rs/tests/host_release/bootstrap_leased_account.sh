#!/bin/sh
# Bootstrap one leased account with the repository's own documented Stado
# installer.
#
# `install-stado.sh` takes its three coordinates from the environment and
# nothing from argv, and `stado host run-attached` forwards arguments but no
# environment. This file is that adapter and nothing else: it exports the
# three documented variables, puts the ordinary system paths in front of the
# installer so `curl`, `jq`, `openssl`, `mktemp` and `tar` resolve for an
# account with no login profile, and then runs the installer unmodified.
#
# Nothing here downloads, verifies, extracts or installs anything. The
# installer beside it does all of that: it fetches the exact immutable
# manifest and archive through `/api/release/object`, checks the manifest's
# five fields, compares the archive against the manifest SHA-256, refuses an
# unexpected or missing member, and installs into `$HOME/.stado/bin` by
# rename. That is the state the release capability calls
# `no-delivery-history`: Stado installed, nothing staged.
set -eu

# The ordinary system paths, first: an account with no login profile has
# none of `dirname`, `curl`, `jq`, `openssl`, `mktemp` or `tar` on its PATH.
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

here="$(cd "$(dirname "$0")" && pwd)"
installer="$here/install-stado.sh"
[ -f "$installer" ] || {
    echo "the documented installer was not delivered beside this file: $installer" >&2
    exit 66
}

STADO_API_URL="${1:?the release channel origin is the first argument}"
STADO_RELEASE_VERSION="${2:?the exact release version is the second argument}"
STADO_RELEASE_PLATFORM="${3:?the release platform is the third argument}"
export STADO_API_URL STADO_RELEASE_VERSION STADO_RELEASE_PLATFORM

exec /bin/sh "$installer"
