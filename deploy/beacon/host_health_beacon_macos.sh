#!/bin/bash
# host_health_beacon_macos.sh — periodic host health writer for the Stado
# backend. Collects launchd unit state for the managed labels and publishes
# it through the authenticated `stado host publish-beacon` control route.
set -euo pipefail

# The `link` block `stado host publish-beacon` collects resolves `pmset`, `log`
# and `tailscale` through PATH, and launchd hands this program the bare
# `/usr/bin:/bin:/usr/sbin:/sbin`. pmset and log are there; tailscale is not on
# any managed Mac -- it ships inside its app bundle and symlinks into
# /usr/local/bin only when somebody clicks "Install CLI". A beacon that cannot
# see tailscale publishes `path_kind: unknown` on a host holding a perfectly
# good direct path, so the directories it actually lives in are named here,
# once, where every launcher of this script picks them up.
PATH="${PATH:-/usr/bin:/bin:/usr/sbin:/sbin}:/usr/local/bin:/opt/homebrew/bin:/Applications/Tailscale.app/Contents/MacOS"
export PATH

STADO_BIN="${STADO_BIN:-$HOME/.stado/bin/stado}"
# The labels this host is judged on, the domains they may live in, and the
# state each one is in are the product's answer now, not this script's:
# `stado host collect-beacon` reads the registry's declarations for this host
# and asks launchd about each of them. What used to be here was a second
# reader -- a console-uid guess, a `gui/<uid>` lookup, a `sudo -n` system
# lookup and a python parse of `registry pull` -- and every one of those
# turned a read it was not allowed to make into the word `inactive`.
HOST_SLUG=$(/bin/hostname -s | /usr/bin/tr '[:upper:]' '[:lower:]')

# Publishing needs the health API, a Skarbiec to mint the bearer against, and
# the consumer grant this host holds. All three are already declared -- the API
# is the store this host is configured to address, and Skarbiec's endpoint is
# in the service directory -- so the product reads them rather than this
# script restating them, and a host that genuinely lacks one fails with its
# name. An installed Stado that predates `beacon-coordinates` answers nothing,
# and the environment this unit was installed with stands.
coordinates=$("$STADO_BIN" host beacon-coordinates --host "$HOST_SLUG" 2>/dev/null || true)
declared_api=${coordinates%%	*}
declared_skarbiec=${coordinates#*	}
[ "$declared_skarbiec" != "$coordinates" ] || declared_skarbiec=''
export STADO_HOST_HEALTH_API_URL="${STADO_HOST_HEALTH_API_URL:-$declared_api}"
# The registry wins over whatever the login environment carries: this host had
# `STADO_HOST_HEALTH_SKARBIEC_URL` pointing at the Weles vault's adapter, for a
# consumer that adapter does not serve, so every publish failed with a refused
# connection while the declared endpoint sat one port away.
export STADO_HOST_HEALTH_SKARBIEC_URL="${declared_skarbiec:-${STADO_HOST_HEALTH_SKARBIEC_URL:-}}"
export STADO_HOST_HEALTH_SKARBIEC_CONSUMER="${STADO_HOST_HEALTH_SKARBIEC_CONSUMER:-stado-host-health-beacon}"
export STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE="${STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE:-$HOME/.stado/host-health-beacon-skarbiec-token}"
printf 'host_health_beacon: api=%s skarbiec=%s collector=%s\n' \
    "$STADO_HOST_HEALTH_API_URL" "$STADO_HOST_HEALTH_SKARBIEC_URL" \
    "$STADO_BIN host collect-beacon" >/dev/stderr

# One collection, in the product: `stado host collect-beacon --publish` reads
# the identities the registry declares for this host, asks the init system
# about each of them through the same reader `stado service label-print` uses,
# and publishes the document through the API this script has just configured.
#
# What used to sit here was a loop that read each label with
# `launchctl print ... 2>/dev/null || true` and called the empty result
# `inactive`. A read the host refuses is also empty, so a daemon loaded in the
# system domain -- every always-on gateway on a Mac -- was published as not
# loaded, and `stado service status`, `registry doctor` and Stado Desktop all
# repeated it. The product now publishes `unreadable` with the cause for a
# refused read, and reads the system domain without asking for privilege it
# does not need.
"$STADO_BIN" host collect-beacon --publish

# Relay for hosts that cannot publish for themselves.
#
# A machine with no stado binary can still collect its own beacon -- that part
# is hostname, df and systemctl -- but it cannot hand it in, and one published
# by hand goes stale within the hour, which is worse than none because it
# still looks like reporting. This host has the binary and the grant, so it
# relays on every tick it already runs: collect over the approved channel,
# publish on that host's behalf.
#
# The list comes from the registry rather than from a name written here. A target
# that publishes for itself is skipped on freshness, not on the absence of a
# collector: `ubuntu-server` has both, its own publisher reports every unit the
# registry declares for it, and the relay's collector carried an older list --
# so relaying on every tick overwrote a correct document with a thinner one, and
# a service that had just been installed and started read as missing for as long
# as the relay kept winning. Ask what the fleet already knows about each host's
# beacon age, and relay only for the ones nobody is reporting.
this_target=$("$STADO_BIN" registry self | { IFS="$(printf '\t')" read -r name _rest || true; printf '%s' "$name"; })
# Seconds after which a reader calls a host health document stale. A host inside
# this window is reporting for itself and must not be spoken over.
READ_FRESH_SECONDS="${WC_BEACON_RELAY_FRESH_SECONDS:-180}"
# Which hosts is nobody reporting? The product asks the store this beacon
# publishes to, not `registry beacon-age`: that command reads through the CLI's
# storage layer, which falls back to a same-disk mirror when the fleet endpoint
# hiccups and then reports hours-old ages for documents that are seconds old. A
# relay driven off those numbers speaks over healthy hosts with a thinner unit
# list than they publish for themselves. A target and its beacon file are also
# spelled differently on a machine named twice, so it tries the name and every
# hostname the registry declares for it. With no override, the targets it
# names are the relay's work list.
stale_targets=$("$STADO_BIN" host beacon-stale --api-url "$STADO_HOST_HEALTH_API_URL" \
    --token-file "${STADO_HOST_HEALTH_API_TOKEN_FILE:-$HOME/.stado/wisent-queue-object-api-token}" \
    --fresh-seconds "$READ_FRESH_SECONDS" 2>/dev/null || printf '')
relay_targets=${WC_BEACON_RELAY_TARGETS:-$stale_targets}
for relay in $relay_targets; do
    [ "$relay" != "$this_target" ] || continue
    # Reporting for itself: leave it alone.
    case " $stale_targets " in
        *" $relay "*) ;;
        *) continue ;;
    esac
    # A host that publishes for itself has no collector under this name, and
    # the failure it returns is expected rather than interesting. Keep the one
    # line this script writes and drop the command's error block, so a tick
    # that worked does not read like a broken one.
    if collected=$("$STADO_BIN" host run-helper "$relay" collect-host-health-beacon 2>/dev/null); then
        printf '%s' "$collected" | /usr/bin/sed -n '/^{/,/^}/p' | "$STADO_BIN" host publish-beacon - >/dev/null \
            || printf '%s\n' "host_health_beacon: publishing on behalf of $relay failed" >/dev/stderr
    else
        printf '%s\n' "host_health_beacon: no collector on $relay; it publishes for itself" >/dev/stderr
    fi
done

# This host's own beacon is published above; the relay is a courtesy for hosts
# that cannot publish for themselves. A target with no collector installed must
# not turn this run into a failure, or the tick that did report reads as broken.
exit 0
