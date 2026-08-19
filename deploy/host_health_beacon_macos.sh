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
# A beacon that runs as a system daemon has no GUI domain of its own: `id -u` is
# 0 and `gui/0` holds nothing, so every user-domain unit read as inactive even
# while it was listening. Ask the console session's domain instead when running
# as root, which is where the fleet's per-user services are loaded.
GUI_UID=$(/usr/bin/id -u)
if [ "$GUI_UID" = "0" ]; then
    CONSOLE_UID=$(/usr/bin/stat -f %u /dev/console 2>/dev/null || printf '')
    case "$CONSOLE_UID" in
        ''|0) : ;;
        *) GUI_UID="$CONSOLE_UID" ;;
    esac
fi
GUI_DOMAIN="gui/$GUI_UID"
# The labels this host is judged on are the ones the registry declares for it.
# A fixed pair written here reported "inactive" for units nobody runs and said
# nothing about the ones that matter.
HOST_SLUG=$(/bin/hostname -s | /usr/bin/tr '[:upper:]' '[:lower:]')
# jq is not on every managed host, and a beacon that dies for want of it is a
# host that reads as dead. python3 ships with macOS and is already what the
# operator helpers use.
# A host knows itself by its hostname; the registry knows it by its target name,
# and the two differ on any machine somebody named twice. `lukaszs-macbook-pro-5485`
# never matched target `lukasz-macbook`, so the label list came back empty, the
# fallback published a map holding only the beacon, and `registry doctor`
# reported a missing plist for six services that were installed and loaded.
# Match the way the fleet's readers match: target name, declared hostnames, or
# either with the local suffix dropped.
READ_LABELS='import json,sys
host = sys.argv[1].lower()
def stem(name):
    name = (name or "").lower()
    return name[:-len(".local")] if name.endswith(".local") else name
doc = json.load(sys.stdin)
for entry in doc.get("targets", []):
    known = {stem(entry.get("name"))}
    known.update(stem(name) for name in entry.get("hostnames", []) or [])
    if stem(host) in known:
        for service in entry.get("services", []):
            print(service.get("label") or service.get("unit") or service.get("name"))
        break'
READ_NAMES='import json,sys
for entry in json.load(sys.stdin).get("targets", []):
    print(entry.get("name"))'
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || printf /usr/bin/python3)}"
LABELS="${WC_HEALTH_UNITS:-$("$STADO_BIN" registry pull 2>/dev/null \
    | "$PYTHON_BIN" -c "$READ_LABELS" "$HOST_SLUG" | /usr/bin/tr '\n' ' ')}"
LABELS="${LABELS:-com.wisent.host-health-beacon}"

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
printf 'host_health_beacon: api=%s skarbiec=%s labels=%s\n' \
    "$STADO_HOST_HEALTH_API_URL" "$STADO_HOST_HEALTH_SKARBIEC_URL" "$LABELS" >/dev/stderr

reported_at=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)
disk_line=$(/bin/df -h / 2>/dev/null | /usr/bin/awk '{line=$0} END {if (line != "") print line}' || true)

units_json=""
for lbl in $LABELS; do
    # An always-on unit lives in the system domain, and a `gui/$uid` lookup
    # misses it entirely -- which is how a running fleet reported itself as
    # inactive. Ask both domains, and let the one that answers decide.
    #
    # A root beacon reading another account's GUI domain gets empty output
    # rather than an error, which read as "inactive" for services that were
    # loaded: root has to enter that session with `asuser` to see it.
    info=$(/bin/launchctl print "${GUI_DOMAIN}/${lbl}" 2>/dev/null || true)
    if [ -z "$info" ] && [ "$(/usr/bin/id -u)" = "0" ]; then
        info=$(/bin/launchctl asuser "$GUI_UID" /bin/launchctl print \
            "${GUI_DOMAIN}/${lbl}" 2>/dev/null || true)
    fi
    if [ -z "$info" ]; then
        info=$(/usr/bin/sudo -n /bin/launchctl print "system/${lbl}" 2>/dev/null || true)
    fi
    if [ -n "$info" ]; then
        state="active"
        # A live process outranks history. launchd keeps the previous run's
        # "last exit code" while the current one is up, so a worker that
        # crashed once and was restarted read as failed for the rest of its
        # life. Read the exit code only when nothing is running now.
        running=$(echo "$info" | /usr/bin/awk '/state = running/ {print "yes"; exit}')
        if [ "$running" != "yes" ]; then
            # "last exit code" is the verdict only when it is a number and not
            # zero; "(never exited)" is healthy, not a failure.
            last_exit=$(echo "$info" | /usr/bin/awk -F'=' '/last exit code/ {gsub(/[ \t]/,""); print $2; exit}')
            if [ -n "$last_exit" ] && [ "$last_exit" != "0" ] && [ "$last_exit" != "(neverexited)" ]; then
                state="failed"
            fi
        fi
    else
        state="inactive"
    fi
    if [ -n "$units_json" ]; then units_json="$units_json,"; fi
    units_json="$units_json\"$lbl\":{\"state\":\"$state\"}"
done

payload="{\"host\":\"$HOST_SLUG\",\"reported_at\":\"$reported_at\",\"disk\":\"$disk_line\",\"units\":{$units_json}}"

"$STADO_BIN" host publish-beacon <(printf '%s' "$payload")

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
