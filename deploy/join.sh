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

# ---------------------------------------------------------------- redeem

log 'Stado fleet enrollment (invite method)'
log "Control address: $api_url"
log ''
log 'Asking the fleet for the public key to install...'

key_body="$work_dir/key.json"
key_status=''
if ! key_status="$(authenticated_curl "$key_body" "$api_url/api/fleet/invite/key")"; then
    die "the control address did not answer: $api_url"
fi
case "$key_status" in
    200) ;;
    401|403|404|409|410) refusal ;;
    *) die "the control plane answered with HTTP $key_status and nothing was changed" ;;
esac

target_name="$(json_string_field target_name <"$key_body")" ||
    die 'the control plane answer did not contain target_name'
authorized_line="$(json_string_field authorized_keys_line <"$key_body")" ||
    die 'the control plane answer did not contain authorized_keys_line'

case "$target_name" in
    ''|*[!A-Za-z0-9._-]*) die 'the control plane sent an unusable target name' ;;
esac
case "$authorized_line" in
    'ssh-ed25519 '*|'ssh-rsa '*|'ecdsa-sha2-'*|'sk-ssh-ed25519@openssh.com '*|'sk-ecdsa-'*) ;;
    *) die 'the control plane sent something that is not an SSH public key' ;;
esac
# A key line is exactly one line. Anything else would smuggle extra directives
# into authorized_keys.
[ "$(printf '%s' "$authorized_line" | awk 'END { print NR }')" = 1 ] ||
    die 'the control plane sent a multi-line key; refusing to install it'

key_type="$(printf '%s' "$authorized_line" | awk '{ print $1 }')"
key_blob="$(printf '%s' "$authorized_line" | awk '{ print $2 }')"
[ -n "$key_blob" ] || die 'the control plane sent a key with no key material'

log "This machine will join the fleet as: $target_name"

# ---------------------------------------------------------------- install key

ssh_dir="$HOME/.ssh"
authorized_keys="$ssh_dir/authorized_keys"

if [ ! -d "$ssh_dir" ]; then
    mkdir -p "$ssh_dir"
fi
chmod 700 "$ssh_dir"
if [ ! -f "$authorized_keys" ]; then
    (umask 077; : >"$authorized_keys")
fi
chmod 600 "$authorized_keys"

# Idempotent on the key material, not on the whole line: the operator may
# re-issue an invitation whose comment differs, and a second run must not leave
# the same key twice.
if awk -v type="$key_type" -v blob="$key_blob" '
        $1 == type && $2 == blob { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$authorized_keys"; then
    key_action='already present'
    log "The fleet key is already in $authorized_keys; leaving it alone."
else
    # Append on its own line even when the file did not end in a newline.
    # Command substitution strips trailing newlines, so a non-empty result
    # means the last byte was something other than a newline.
    if [ -s "$authorized_keys" ] && [ -n "$(tail -c 1 "$authorized_keys")" ]; then
        printf '\n' >>"$authorized_keys"
    fi
    printf '%s\n' "$authorized_line" >>"$authorized_keys"
    key_action='installed'
    log "Installed the fleet key in $authorized_keys."
fi

# SHA256 fingerprint of the installed key, so the operator can confirm at
# approval time that this machine holds the key the fleet minted.
fingerprint=''
if command -v ssh-keygen >/dev/null 2>&1; then
    printf '%s\n' "$authorized_line" >"$work_dir/fleet.pub"
    fingerprint="$(ssh-keygen -lf "$work_dir/fleet.pub" 2>/dev/null | awk '{ print $2 }')" || fingerprint=''
fi
if [ -z "$fingerprint" ] && command -v openssl >/dev/null 2>&1; then
    digest="$(printf '%s' "$key_blob" |
        openssl base64 -d -A 2>/dev/null |
        openssl dgst -sha256 -binary 2>/dev/null |
        openssl base64 -A 2>/dev/null)" || digest=''
    if [ -n "$digest" ]; then
        fingerprint="SHA256:${digest%%=*}"
    fi
fi
case "$fingerprint" in
    SHA256:*) ;;
    *) fingerprint='' ;;
esac

# ---------------------------------------------------------------- reachability

os_name="$(uname -s)"
machine_arch="$(uname -m)"
login_user="$(id -un)"
short_hostname="$(uname -n | awk '{ sub(/\..*$/, "", $0); print tolower($0) }')"
[ -n "$short_hostname" ] || die 'this machine does not report a hostname'

tailscale_bin=''
if command -v tailscale >/dev/null 2>&1; then
    tailscale_bin="$(command -v tailscale)"
else
    for candidate in \
        /Applications/Tailscale.app/Contents/MacOS/Tailscale \
        /usr/local/bin/tailscale \
        /opt/homebrew/bin/tailscale
    do
        if [ -x "$candidate" ]; then
            tailscale_bin="$candidate"
            break
        fi
    done
fi

address=''
address_kind=''
if [ -n "$tailscale_bin" ]; then
    # --peers=false leaves exactly this machine's own record, so the DNSName
    # read back cannot be some other node's.
    tailnet_name="$("$tailscale_bin" status --json --peers=false 2>/dev/null |
        json_string_field DNSName 2>/dev/null || true)"
    tailnet_name="${tailnet_name%.}"
    case "$tailnet_name" in
        ''|*[!A-Za-z0-9.-]*) ;;
        *)
            address="$tailnet_name"
            address_kind='tailnet name'
            ;;
    esac
fi

if [ -z "$address" ]; then
    case "$os_name" in
        Darwin)
            local_name="$(scutil --get LocalHostName 2>/dev/null || true)"
            if [ -n "$local_name" ]; then
                address="$local_name.local"
                address_kind='multicast DNS name'
            fi
            ;;
        Linux)
            # Only claim .local where something actually answers for it.
            if [ -S /run/avahi-daemon/socket ] || [ -S /var/run/avahi-daemon/socket ]; then
                address="$short_hostname.local"
                address_kind='multicast DNS name'
            fi
            ;;
    esac
fi

if [ -z "$address" ]; then
    case "$os_name" in
        Darwin)
            default_if="$(route -n get default 2>/dev/null |
                awk '$1 == "interface:" { print $2; exit }')"
            if [ -n "$default_if" ]; then
                address="$(ipconfig getifaddr "$default_if" 2>/dev/null || true)"
            fi
            ;;
        Linux)
            address="$(ip route get 1.1.1.1 2>/dev/null |
                awk '{ for (i = 1; i < NF; i++) if ($i == "src") { print $(i + 1); exit } }')"
            if [ -z "$address" ] && command -v hostname >/dev/null 2>&1; then
                address="$(hostname -I 2>/dev/null | awk '{ print $1 }')"
            fi
            ;;
    esac
    if [ -n "$address" ]; then
        address_kind='IPv4 address of the default interface'
    fi
fi

if [ -z "$address" ]; then
    address="$short_hostname"
    address_kind='bare hostname (nothing better was resolvable)'
fi

destination="$login_user@$address"

# ---------------------------------------------------------------- sshd probe

# The fleet dials in over SSH, so a machine with no SSH server answering is
# reachable in name only. Remote Login is the owner's decision and needs
# administrator rights: diagnose it, print the exact way to turn it on, and
# never turn it on silently.
ssh_listening='unknown'
if command -v nc >/dev/null 2>&1; then
    if nc -z -w 3 127.0.0.1 22 >/dev/null 2>&1; then
        ssh_listening='yes'
    else
        ssh_listening='no'
    fi
elif command -v ssh >/dev/null 2>&1; then
    ssh_probe="$(ssh -o BatchMode=yes -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 \
        127.0.0.1 true 2>&1 || true)"
    case "$ssh_probe" in
        *'Connection refused'*|*'onnection timed out'*|*'No route to host'*) ssh_listening='no' ;;
        *) ssh_listening='yes' ;;
    esac
fi

ssh_instructions() {
    case "$os_name" in
        Darwin)
            cat <<'MACOS'
Turn on Remote Login yourself -- it needs administrator rights, so this script
will not do it for you:

  System Settings > General > Sharing > Remote Login  (switch it on, and under
  the (i) button allow access for your own user)

The equivalent from a terminal, which will ask for your password:

  sudo systemsetup -setremotelogin on
MACOS
            ;;
        Linux)
            cat <<'LINUX'
Start an SSH server yourself -- it needs root, so this script will not do it
for you. On Debian/Ubuntu:

  sudo apt install openssh-server
  sudo systemctl enable --now ssh

On Fedora/RHEL/Arch:

  sudo dnf install openssh-server   # or: sudo pacman -S openssh
  sudo systemctl enable --now sshd

Then make sure the host firewall lets port 22 through from the fleet.
LINUX
            ;;
        *)
            cat <<'OTHER'
Start an SSH server on this machine (port 22) and let the fleet reach it. This
script will not start one for you.
OTHER
            ;;
    esac
}

# ---------------------------------------------------------------- report in

log ''
log "Reporting this machine to the fleet as $destination ..."

# The five contract fields, plus `ssh_listening` so the operator sees before
# approving that the channel does not answer yet. A probe that could not be
# made counts as not answering: claiming a channel works when nothing verified
# it is the one lie that would waste the operator's approval attempt.
join_body="$work_dir/join.json"
cat >"$join_body" <<JSON
{
  "hostname": "$short_hostname",
  "os": "$os_name",
  "arch": "$machine_arch",
  "destination": "$destination",
  "installed_key_fingerprint": "$fingerprint",
  "ssh_listening": $( [ "$ssh_listening" = yes ] && printf true || printf false )
}
JSON

join_status=''
if ! join_status="$(authenticated_curl "$work_dir/join-response.json" \
    --request POST \
    --header 'Content-Type: application/json' \
    --data @"$join_body" \
    "$api_url/api/fleet/join")"; then
    die "the control address did not answer: $api_url"
fi
case "$join_status" in
    200|201|202) ;;
    401|403|404|409|410) refusal ;;
    *) die "the control plane answered with HTTP $join_status; this machine was not reported" ;;
esac

# ---------------------------------------------------------------- summary

log ''
log '--------------------------------------------------------------'
log 'What just happened'
log '--------------------------------------------------------------'
log "  Fleet name for this machine: $target_name"
log "  Fleet key ($key_type): $key_action in $authorized_keys"
if [ -n "$fingerprint" ]; then
    log "  Key fingerprint: $fingerprint"
else
    log '  Key fingerprint: not computable here; the operator will verify the'
    log '    key when approving this machine'
fi
log "  Reported address: $destination"
log "    Chosen as the $address_kind."
log '    If the fleet reaches this machine at a different address, tell the'
log '    operator -- they set the final address when approving, and nothing'
log '    has to be redone here.'
log '  Invitation: redeemed (the code is now spent, and was never written to'
log '    this machine)'
log ''
case "$ssh_listening" in
    yes)
        log 'Remote login: an SSH server is answering on port 22.'
        ;;
    no)
        log 'Remote login: NOTHING is answering on port 22, so the fleet cannot'
        log 'reach this machine yet. This machine was still reported, marked as'
        log 'a channel that does not answer.'
        log ''
        ssh_instructions
        ;;
    *)
        log 'Remote login: could not be checked on this machine (no nc, no ssh'
        log 'client). The fleet needs an SSH server answering on port 22; this'
        log 'machine was still reported.'
        log ''
        ssh_instructions
        ;;
esac
log ''
log 'What is left, and who does it'
log '  Stado itself was NOT installed here, on purpose. The operator installs'
log '  the agent when they approve this machine, over the SSH channel the key'
log '  above just opened.'
log '  The operator now runs, on their own machine:'
log "    stado fleet pending"
log "    stado fleet approve $short_hostname"
log '  Nothing further is needed from you.'
