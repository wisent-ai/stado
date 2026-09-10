#!/bin/sh
# join.sh -- the `invite` enrollment method, run by the OWNER of the machine
# being added to the Stado fleet, on a machine that has no Stado at all:
#
#     curl -fsSL https://stado.wisent.com/join.sh | sh -s -- <invitation-code>
#
# What this script does, and nothing more:
#   1. redeems the invitation for the fleet's PUBLIC key,
#   2. installs that public key into this machine's ~/.ssh/authorized_keys,
#   3. reports this machine (hostname, os, arch, reachable address) as a
#      pending enrollment for the operator to approve.
#
# The key direction is fixed and not reversible: the fleet dials IN to this
# machine, so this machine receives a PUBLIC key and never sees, generates or
# transmits a private one. The invitation code is read from argv, is sent only
# as a bearer token over the control channel, and is never printed, logged, or
# written to a file.
#
# DO NOT ADD A STADO INSTALLER HERE. Installing the agent is deliberately not
# part of this script: the operator installs it during `stado fleet approve`,
# which probes this machine and bootstraps it over the SSH channel the key
# above just opened. An installer here would run unreviewed release code on a
# stranger's laptop before any operator ever approved the machine, and would
# duplicate -- badly -- the probe-then-write ordering `approve` already has.

set -eu

DEFAULT_API_URL='https://stado.wisent.com'

log() {
    printf '%s\n' "$*"
}

warn() {
    printf '%s\n' "$*" >&2
}

die() {
    printf 'join: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat >&2 <<'USAGE'
usage: curl -fsSL https://stado.wisent.com/join.sh | sh -s -- <invitation-code> [control-url]

  <invitation-code>  the one-line code the fleet operator sent you
  [control-url]      control address, when the operator gave you a different
                     one (or set STADO_API_URL in the environment)
USAGE
    exit 2
}

# ---------------------------------------------------------------- arguments

[ "$#" -ge 1 ] || usage
invite_token="$1"
shift
api_url="${STADO_API_URL:-${1:-$DEFAULT_API_URL}}"
api_url="${api_url%/}"

# `<id>.<secret>`: 16 hex characters, then 32 random bytes in unpadded
# base64url. Checked here so a mistyped code fails saying so, instead of
# reaching the control plane and coming back as an indistinguishable refusal.
# The code itself is never echoed, not even in these errors.
case "$invite_token" in
    *.*) ;;
    *) die 'the invitation code is malformed (expected <id>.<secret>)' ;;
esac
invite_id="${invite_token%%.*}"
invite_secret="${invite_token#*.}"
case "$invite_id" in
    ''|*[!0-9a-f]*) die 'the invitation code is malformed (bad identifier)' ;;
esac
[ "${#invite_id}" -eq 16 ] || die 'the invitation code is malformed (bad identifier length)'
case "$invite_secret" in
    ''|*[!A-Za-z0-9_-]*) die 'the invitation code is malformed (bad secret)' ;;
esac
[ "${#invite_secret}" -eq 43 ] || die 'the invitation code is malformed (bad secret length)'

# The code travels as a bearer token, so the control channel must be encrypted.
# Loopback is exempt: there is no network to eavesdrop, and that is how a local
# dashboard is exercised.
case "$api_url" in
    https://*) ;;
    http://127.0.0.1|http://127.0.0.1:*|http://localhost|http://localhost:*|http://\[::1\]|http://\[::1\]:*) ;;
    http://*) die "the control address must use HTTPS: $api_url" ;;
    *) die "the control address must be an http(s) URL: $api_url" ;;
esac

# No ssh-keygen in this list: this machine never generates a key pair, it only
# receives the fleet's public key.
for required in curl awk cat mkdir chmod mktemp rm tail uname id; do
    command -v "$required" >/dev/null 2>&1 ||
        die "this machine is missing a required command: $required"
done

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/stado-join.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM

# ---------------------------------------------------------------- helpers

# One string field out of a flat JSON object read from stdin. jq is not on a
# fresh macOS, and neither is a python3 that answers without Xcode, so the
# parse is done with the awk every POSIX system has.
json_string_field() {
    awk -v field="$1" '
        { doc = doc $0 "\n" }
        END {
            at = index(doc, "\"" field "\"")
            if (at == 0) exit 1
            rest = substr(doc, at + length(field) + 2)
            if (!sub(/^[ \t\r\n]*:[ \t\r\n]*/, "", rest)) exit 1
            if (substr(rest, 1, 1) != "\"") exit 1
            rest = substr(rest, 2)
            total = length(rest)
            for (i = 1; i <= total; i++) {
                c = substr(rest, i, 1)
                if (c == "\\") {
                    i += 1
                    e = substr(rest, i, 1)
                    if (e == "n") value = value "\n"
                    else if (e == "t") value = value "\t"
                    else if (e == "r") value = value "\r"
                    else if (e == "u") exit 1
                    else value = value e
                } else if (c == "\"") {
                    print value
                    exit 0
                } else {
                    value = value c
                }
            }
            exit 1
        }
    '
}

# curl, with the invitation code handed over on stdin as a config file so it
# never appears in this machine's process list and never lands on its disk.
# Prints the HTTP status; writes the response body to $1.
authenticated_curl() {
    body_path="$1"
    shift
    printf 'header = "Authorization: Bearer %s"\n' "$invite_token" |
        curl --silent --show-error --location --config - \
            --max-time 30 --output "$body_path" --write-out '%{http_code}' "$@"
}

refusal() {
    cat >&2 <<'REFUSAL'
join: the invitation was refused.

An invitation is refused when it has already been used, has expired, was
revoked, or was never issued -- the control plane deliberately does not say
which. Nothing was changed on this machine.

Ask the fleet operator for a fresh invitation code.
REFUSAL
    exit 1
}

