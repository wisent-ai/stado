#!/usr/bin/env bash
# Prove that scripts/version_check.sh actually refuses. A gate nobody has watched say
# no is indistinguishable from a gate that always says yes, and this one guards
# create-only release objects, so the refusals are the part worth checking.
#
# Five cases, run against the committed baseline and a built binary:
#   1 surface untouched                     -> internal, passes
#   2 one advertised command removed        -> breaking, refuses
#   3 one advertised command added          -> additive, refuses
#   4 "source" is prose, not a marker       -> refuses (unknown baseline marker)
#   5 baseline claims an object the channel does not serve -> refuses
#
# Cases 2 and 3 mutate a copy of the candidate surface; cases 4 and 5 a copy of the
# baseline. The repository is never written to. This needs no CI: Actions must not be
# the only place the gate has ever been observed.
#
# One arm is deliberately not automated here, because automating it would write to the
# repository: the staleness refusal, which fires when a better artifact than the
# baseline's becomes reachable. Exercise it by hand and put the tag back:
#   git tag zz-probe HEAD && scripts/version_check_proof.sh; git tag -d zz-probe
# With the tag present the gate must refuse case 1 with "the baseline is head, but
# git-archive:zz-probe is reachable now"; without it, case 1 must pass.
#
# Usage:
#   scripts/version_check_proof.sh [stado-binary]
# Environment:
#   AUTOVERSION  the shared rule's entry point  (default: autoversion on PATH)
#   STADO_BIN    stado used to read the channel (default: ~/.stado/bin/stado)

set -uo pipefail

repository="$(cd "$(dirname "$0")/.." && pwd)" || exit
cd "$repository" || exit

binary="${1:-stado-rs/target/release/stado}"
export AUTOVERSION="${AUTOVERSION:-autoversion}"
export STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=""

# check_case <name> <pass|refuse> <baseline> <candidate>
check_case() {
  local name="$1" expected="$2" baseline="$3" candidate="$4" observed
  echo
  echo "### $name (expect $expected)"
  if BASELINE="$baseline" scripts/version_check.sh "$candidate"; then
    observed=pass
  else
    observed=refuse
  fi
  if [ "$observed" = "$expected" ]; then
    echo "OK: $name -> $observed"
  else
    echo "FAIL: $name -> $observed, expected $expected"
    failures="${failures}x"
  fi
}

python3 scripts/surface.py --binary "$binary" > "$work/candidate.json" || exit
removed="$(jq -r '.surface | last' "$work/candidate.json")"
echo "candidate surface: $(jq -c '.surface | length' "$work/candidate.json") commands"
jq -c .surface "$work/candidate.json"

jq --arg gone "$removed" '{surface: (.surface | map(select(. != $gone)))}' \
  "$work/candidate.json" > "$work/removed.json"
jq '{surface: ((.surface + ["teleport"]) | sort)}' \
  "$work/candidate.json" > "$work/added.json"
jq '.source = "handwritten by someone in a hurry"' released-surface.json \
  > "$work/prose.json"
jq '.source = "stado:stado/9.9.9/nowhere-noarch/stado published to the Stado channel"' \
  released-surface.json > "$work/absent-object.json"

check_case "surface untouched" pass released-surface.json "$work/candidate.json"
check_case "removed $removed" refuse released-surface.json "$work/removed.json"
check_case "added teleport" refuse released-surface.json "$work/added.json"
check_case "prose baseline source" refuse "$work/prose.json" "$work/candidate.json"
check_case "baseline claims absent object" refuse "$work/absent-object.json" \
  "$work/candidate.json"

echo
if [ -n "$failures" ]; then
  echo "the gate did not behave as specified"
  false
else
  echo "every case behaved as specified"
fi
