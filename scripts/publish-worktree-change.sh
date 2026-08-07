#!/bin/sh
# Publish changes from this checkout when its worktree metadata is unusable.
#
# The Documents checkout is a git worktree whose administrative directory lived
# under the CI runner's _work tree and was cleaned away, so every git command run
# here dies with "not a repository". Rather than hand-rebuild that metadata --
# fragile, and easy to get subtly wrong -- this clones the branch fresh, copies the
# named files over, and commits there. The clone is discarded afterwards, so the
# only lasting effect is the commit on the remote branch.
#
# Environment:
#   BRANCH   remote branch to publish onto
#   MESSAGE  commit message
#   PATHS    space-separated repository-relative paths to copy
set -eu

: "${BRANCH:?BRANCH is required}"
: "${MESSAGE:?MESSAGE is required}"
: "${PATHS:?PATHS is required}"

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git clone -q --branch "$BRANCH" https://github.com/wisent-ai/stado.git "$work/stado"
for path in $PATHS; do
  cp "$root/$path" "$work/stado/$path"
done
cd "$work/stado"
git add -- $PATHS
git -c user.email=lgb2127@columbia.edu -c user.name=lbartoszcze commit -q -m "$MESSAGE"
git push -q origin "HEAD:$BRANCH"
git rev-parse --short HEAD
