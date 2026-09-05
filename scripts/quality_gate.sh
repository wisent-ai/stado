#!/usr/bin/env bash
# Run the quality steps the release manifest declares, and say WHOSE revision
# a refusal belongs to.
#
# The definition of "good" is not restated here. `.wisent-release.json` is the
# product's release-pipeline manifest: `stado release submit` reads
# `platforms.<platform>.quality` and the release worker executes exactly those
# argv before it builds. This script reads the same key out of the same file
# and executes the same argv, so the gate cannot drift from the one it stands
# in for.
#
# What it adds is attribution. A pull request is judged on its MERGE result, so
# a step that already refuses the base branch refuses every pull request opened
# against it — for files the author never touched. On 2026-09-04 and 2026-09-05
# that happened three times in one day on this repository: `main` carried
# unformatted `cli/onboarding.rs`, then `cli/identity.rs`, then
# `dashboard/mod.rs`, and each time the gate told the author of an unrelated
# change to "fix the tree". Three authors reformatted somebody else's code to
# get their own work through, and the revision that introduced it was never
# named. So on a refusal this script runs the same step against the base
# revision in its own worktree and reports one of two verdicts:
#
#   introduced  this revision introduces the failure
#   inherited   the base already fails it, with the base sha
#
# Usage: scripts/quality_gate.sh [--manifest <path>] [--base <ref>]
#
# `--base` defaults to `origin/main` when it resolves and is skipped when it
# does not: a repository with no base to compare against still gets the gate,
# it just gets no attribution.
set -euo pipefail

manifest=".wisent-release.json"
base="origin/main"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --manifest) manifest="${2:?--manifest needs a path}"; shift 2 ;;
    --base) base="${2:?--base needs a ref}"; shift 2 ;;
    --help|-h)
      /usr/bin/sed -n '2,29p' "$0" | /usr/bin/sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) printf '::error::unknown option %s\n' "$1" >&2; exit 64 ;;
  esac
done

if [ ! -f "$manifest" ]; then
  printf '::error::%s does not exist, so there is no declared gate to run\n' "$manifest" >&2
  exit 1
fi

# The runner's own platform, not a hardcoded one, so moving this to a macOS
# runner runs the darwin-arm64 steps rather than silently judging the wrong
# ones.
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-amd64 ;;
  Darwin-arm64) platform=darwin-arm64 ;;
  *)
    printf '::error::%s declares no release platform for %s-%s\n' \
      "$manifest" "$(uname -s)" "$(uname -m)" >&2
    exit 1 ;;
esac
printf 'platform: %s\n' "$platform"

# A gate that can only say yes is worth nothing, and the cheapest way to reach
# that state is to empty the array this reads. An absent platform, an absent
# `quality` key or an empty one is a refusal, not a pass with zero steps.
steps="$(jq -er --arg p "$platform" '.platforms[$p].quality // [] | length' "$manifest")"
if [ "$steps" -lt 1 ]; then
  printf '::error::%s declares %s quality step(s) for %s. With nothing declared there is no gate, and a green tick here would be a lie. Restore the declaration or delete this gate deliberately.\n' \
    "$manifest" "$steps" "$platform" >&2
  exit 1
fi
printf '%s declares %s quality step(s) for %s\n' "$manifest" "$steps" "$platform"

step_argv() {
  # NUL-delimited, so an argv element containing a space or a glob reaches the
  # program the way the release worker passes it, not the way a shell would
  # re-split it.
  jq -j --arg p "$platform" --argjson i "$1" \
    '.platforms[$p].quality[$i].argv[] | . + "\u0000"' "$manifest"
}

base_sha=""
if git rev-parse --verify --quiet "$base^{commit}" >/dev/null 2>&1; then
  base_sha="$(git rev-parse "$base^{commit}")"
fi

# Whether the same step also refuses the base revision, run in the base's own
# worktree so the answer is about that revision's files and nothing else.
#
# A base that cannot be checked out is not evidence either way: the step's own
# verdict stands and the report says the base was not consulted.
inherited() {
  local index="$1" checkout status
  [ -n "$base_sha" ] || return 2
  checkout="$(mktemp -d)"
  if ! git worktree add --detach --quiet "$checkout" "$base_sha" >/dev/null 2>&1; then
    /bin/rm -rf "$checkout"
    return 2
  fi
  local argv=()
  while IFS= read -r -d '' element; do argv+=("$element"); done < <(step_argv "$index")
  status=0
  ( cd "$checkout" && "${argv[@]}" ) >/dev/null 2>&1 || status=$?
  git worktree remove --force "$checkout" >/dev/null 2>&1 || /bin/rm -rf "$checkout"
  [ "$status" -ne 0 ]
}

index=0
while [ "$index" -lt "$steps" ]; do
  name="$(jq -er --arg p "$platform" --argjson i "$index" \
    '.platforms[$p].quality[$i].name' "$manifest")"
  argv=()
  while IFS= read -r -d '' element; do argv+=("$element"); done < <(step_argv "$index")
  if [ "${#argv[@]}" -lt 1 ]; then
    printf "::error::quality step '%s' declares no argv\n" "$name" >&2
    exit 1
  fi
  printf '::group::%s: %s\n' "$name" "${argv[*]}"
  if ! "${argv[@]}"; then
    printf '::endgroup::\n'
    verdict=0
    inherited "$index" || verdict=$?
    case "$verdict" in
      0)
        printf "::error::verdict=inherited step='%s' base=%s: the release quality step refuses this revision AND already refuses %s at %s, so this pull request did not introduce it and reformatting here fixes somebody else's revision. Repair %s in its own change; this gate stays red until that lands.\n" \
          "$name" "$base_sha" "$base" "$base_sha" "$base" >&2 ;;
      2)
        printf "::error::verdict=unattributed step='%s': the release quality step refuses this revision and the base (%s) could not be checked out to compare, so whose revision carries it is unknown. Fix the tree; do not narrow the step.\n" \
          "$name" "$base" >&2 ;;
      *)
        printf "::error::verdict=introduced step='%s': the release quality step refuses this revision and passes on %s at %s, so this change introduces it. It is declared in %s and the release worker runs the same argv, so this is the failure the release would have hit -- with a version already spent on it. Fix the tree; do not narrow the step.\n" \
          "$name" "$base" "$base_sha" "$manifest" >&2 ;;
    esac
    exit 1
  fi
  printf '::endgroup::\n'
  index=$((index + 1))
done

printf 'Every quality step %s declares for %s passed.\n' "$manifest" "$platform"
