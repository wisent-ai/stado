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
# jq is not on every managed host, and a beacon that dies for want of it is a
# host that reads as dead. python3 ships with macOS and is already what the
# operator helpers use.
READ_NAMES='import json,sys
for entry in json.load(sys.stdin).get("targets", []):
    print(entry.get("name"))'
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || printf /usr/bin/python3)}"

# Publishing needs the health API, a Skarbiec to mint the bearer against, and
# the consumer grant this host holds. All three are already declared -- the API
# is the store this host is configured to address, and Skarbiec's endpoint is
# in the service directory -- so read them rather than restate them, and let a
# host that genuinely lacks one fail with its name.
READ_STORE_URL='import json,pathlib
p = pathlib.Path.home() / ".config" / "stado" / "config.json"
print(json.loads(p.read_text()).get("storage", {}).get("stado", {}).get("url", "") if p.is_file() else "")'
READ_SKARBIEC='import json,sys
host = sys.argv[1]
doc = json.load(sys.stdin)
service = doc.get("service_directory", {}).get("services", {}).get("skarbiec", {})
print(service.get("endpoints", {}).get(host, {}).get("url", ""))'
export STADO_HOST_HEALTH_API_URL="${STADO_HOST_HEALTH_API_URL:-$("$PYTHON_BIN" -c "$READ_STORE_URL")}"
# The registry wins over whatever the login environment carries: this host had
# `STADO_HOST_HEALTH_SKARBIEC_URL` pointing at the Weles vault's adapter, for a
# consumer that adapter does not serve, so every publish failed with a refused
# connection while the declared endpoint sat one port away.
declared_skarbiec=$("$STADO_BIN" registry pull 2>/dev/null \
    | "$PYTHON_BIN" -c "$READ_SKARBIEC" "$HOST_SLUG")
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
relay_targets=${WC_BEACON_RELAY_TARGETS:-$("$STADO_BIN" registry pull | "$PYTHON_BIN" -c "$READ_NAMES")}
# Seconds after which a reader calls a host health document stale. A host inside
# this window is reporting for itself and must not be spoken over.
READ_FRESH_SECONDS="${WC_BEACON_RELAY_FRESH_SECONDS:-180}"
RELAY_TOKEN=$(/bin/cat "${STADO_HOST_HEALTH_API_TOKEN_FILE:-$HOME/.stado/wisent-queue-object-api-token}" 2>/dev/null || printf '')
# Which hosts is nobody reporting? Ask the store this beacon publishes to, not
# `registry beacon-age`: that command reads through the CLI's storage layer,
# which falls back to a same-disk mirror when the fleet endpoint hiccups and then
# reports hours-old ages for documents that are seconds old. A relay driven off
# those numbers speaks over healthy hosts with a thinner unit list than they
# publish for themselves. A target and its beacon file are also spelled
# differently on a machine named twice, so try the name and every hostname the
# registry declares for it.
READ_STALE='import datetime, json, sys, urllib.parse, urllib.request
base, token, limit = sys.argv[1].rstrip("/"), sys.argv[2], float(sys.argv[3])
document = json.load(sys.stdin)
now = datetime.datetime.now(datetime.timezone.utc)
def age(slug):
    uri = "stado://probierz/host_health/%s.json" % slug
    url = "%s/api/object?uri=%s" % (base, urllib.parse.quote(uri, safe=""))
    request = urllib.request.Request(url, headers={"Authorization": "Bearer %s" % token})
    try:
        body = json.load(urllib.request.urlopen(request, timeout=10))
        stamp = (body.get("reported_at") or "").replace("Z", "+00:00")
        return (now - datetime.datetime.fromisoformat(stamp)).total_seconds()
    except Exception:
        return None
stale = []
for entry in document.get("targets", []):
    name = entry.get("name") or ""
    spellings = [name] + [h.lower().removesuffix(".local") for h in entry.get("hostnames", []) or []]
    ages = [value for value in (age(slug) for slug in dict.fromkeys(spellings)) if value is not None]
    if not ages or min(ages) >= limit:
        stale.append(name)
print(" ".join(stale))'
stale_targets=$("$STADO_BIN" registry pull 2>/dev/null \
    | "$PYTHON_BIN" -c "$READ_STALE" "$STADO_HOST_HEALTH_API_URL" "$RELAY_TOKEN" "$READ_FRESH_SECONDS" 2>/dev/null || printf '')
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
