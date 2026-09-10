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
