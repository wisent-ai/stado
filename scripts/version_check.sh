#!/usr/bin/env bash
# Refuse a release whose declared version disagrees with what it did to the public
# contract of `stado`. That contract is the command list the binary advertises: adding
# a command is a capability, removing one breaks every script that invoked it.
#
# The rule itself has one home for the whole fleet and is not copied here:
# https://github.com/lbartoszcze/AutoVersion. This script supplies only the three
# things this repository alone knows — the surface of the artifact about to be
# published, the version that artifact declares, and which release channel it is
# published to.
#
# Run by .github/workflows/deploy.yml in front of the first `storage put`. Release
# objects are create-only, so a wrong version is not a mistake anyone can correct
# afterwards; the only useful place for this check is ahead of the publish it guards.
#
# Usage:
#   scripts/version_check.sh <candidate-surface.json>
# Environment:
#   AUTOVERSION  the shared rule's entry point   (default: autoversion on PATH)
#   STADO_BIN    stado used to read the channel  (default: ~/.stado/bin/stado)
#   BASELINE     the published baseline document (default: released-surface.json)
#   MANIFEST     the crate that declares version (default: stado-rs/Cargo.toml)

set -euo pipefail

candidate="${1:?usage: scripts/version_check.sh <candidate-surface.json>}"
baseline="${BASELINE:-released-surface.json}"
manifest="${MANIFEST:-stado-rs/Cargo.toml}"
autoversion="${AUTOVERSION:-autoversion}"
stado_bin="${STADO_BIN:-$HOME/.stado/bin/stado}"

test -r "$candidate"
test -r "$baseline"
test -r "$manifest"
test -x "$stado_bin"

# Read the declared version exactly the way deploy.yml reads it, so the gate and the
# publish can never disagree about what is being released.
version_line="$(sed -n '/^version = /p' "$manifest")"
declared="${version_line#*\"}"
declared="${declared%%\"*}"
released="$(jq -r .version "$baseline")"
test -n "$declared"
test -n "$released"
echo "declared version: $declared"
echo "baseline version: $released"

verdict="$("$autoversion" decide \
  --current "$released" \
  --published-surface "$baseline" \
  --candidate-surface "$candidate" \
  --json)"
echo "$verdict"

change="$(printf '%s' "$verdict" | jq -r .change)"
required="$(printf '%s' "$verdict" | jq -r .next)"
test -n "$change"
test -n "$required"

if [ "$declared" = "$released" ]; then
  if [ "$change" != internal ]; then
    echo "::error::The advertised command surface changed ($change), but" \
      "$manifest still declares the baseline version $released." \
      "The next version must be $required."
    false
  fi
  echo "The surface is unchanged, so the declared version may stay $released."
elif [ "$declared" != "$required" ]; then
  echo "::error::$manifest declares $declared, but a $change change to" \
    "$released requires $required."
  false
else
  echo "Declared version $declared matches the $change change."
fi

# The baseline must not lie about the channel, in either direction. Checked against
# the channel this product actually publishes to; asserting against a registry it was
# never published to would pass vacuously and prove nothing. The marker is the first
# whitespace-delimited token of "source" and is written by scripts/baseline.py.
marker="$(jq -r '.source | split(" ") | first' "$baseline")"
echo "baseline marker: $marker"
case "$marker" in
  stado:*) claims_channel=yes ;;
  git-archive:* | head:*) claims_channel=no ;;
  *)
    echo "::error::unknown baseline marker in $baseline: $marker." \
      "Regenerate it: python3 scripts/baseline.py"
    false
    ;;
esac

listing="$("$stado_bin" storage objects releases stado/ --json)"
if [ "$claims_channel" = yes ]; then
  key="${marker#stado:}"
  if ! printf '%s' "$listing" |
    jq -e --arg key "$key" '.objects | any(.key == $key)' >/dev/null; then
    echo "::error::$baseline claims the published object $key, which the release" \
      "channel does not serve. Regenerate it: python3 scripts/baseline.py"
    false
  fi
  echo "The channel serves $key, as the baseline claims."
else
  # An empty listing and an unreachable store are the same silence, and here the wrong
  # answer is the passing one: the assertion would conclude "nothing is published"
  # precisely when it learned nothing. So absence is only read after a request that
  # demonstrably succeeded. `stat` is the control because it distinguishes absent from
  # unreachable, and the probe object need not exist. The forward branch above needs no
  # such control: it requires a positive answer, so silence there already refuses.
  probe="stado://releases/stado/$released/linux-amd64/stado"
  probe_state="$("$stado_bin" storage stat "$probe" --json | jq -r .state)"
  if [ "$probe_state" = unreachable ]; then
    echo "::error::the release channel is unreachable ($probe), so the absence of a" \
      "published stado release is unproven. Refusing rather than assuming that" \
      "$marker is honest."
    false
  fi
  if printf '%s' "$listing" | jq -e '.objects | any(has("key"))' >/dev/null; then
    echo "::error::$baseline claims nothing is published ($marker), but the release" \
      "channel already serves stado releases, so every comparison is measured" \
      "against the wrong artifact. Regenerate it: python3 scripts/baseline.py"
    false
  fi
  echo "The channel is readable and serves no stado release, as the baseline claims."
fi

# Being honest about the marker is not the same as sitting on the best artifact. A
# tag, a newer release, or a first publication can appear after the baseline was
# generated; the marker stays truthful while the artifact it names is superseded, and
# the comparison quietly goes on measuring the worse thing.
#
# Identities, not markers, are compared. A published release is identified by its
# version, because the object key also names a platform and this same baseline is
# checked from hosts of different platforms. The last-resort tier is identified by its
# name alone, because a head sha moves with every commit and full equality there would
# demand a regenerated baseline per commit, forever.
#
# The generator is asked for the identity only and recovers no surface: a regenerated
# surface must never reach the decision, or the check compares the tree against itself
# and can refuse nothing.
case "$marker" in
  stado:*)
    key="${marker#stado:}"
    published_version="${key#*/}"
    have="stado:${published_version%%/*}"
    ;;
  git-archive:*) have="$marker" ;;
  head:*) have="head" ;;
esac
best="$(python3 scripts/baseline.py --best --stado "$stado_bin")"
if [ "$best" != "$have" ]; then
  echo "::error::the baseline is $have, but $best is reachable now, so every" \
    "comparison is measured against a superseded artifact." \
    "Regenerate it: python3 scripts/baseline.py"
  false
fi
echo "$have is still the best artifact available."
