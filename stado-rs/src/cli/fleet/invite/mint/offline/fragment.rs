//! One literal, on its own, so that reading the fragment reads exactly what
//! the far machine runs.

/// The offline fragment, verbatim, with `@TARGET@` and `@FLEET_KEY@` left to
/// substitute. One literal so that what an operator reads before sending it is
/// exactly what runs on the far machine.
///
/// It wraps itself in `sh <<'...'`: the fragment is pasted into whatever
/// interactive shell the owner already has open, and a body that runs under
/// `set -eu` and calls `exit` on a missing tool must not be able to close that
/// shell.
///
/// The address rules are `deploy/join.sh`'s, in its order — tailnet DNS name,
/// then a multicast `.local` name only where something answers for it, then the
/// IPv4 address of the default interface, then the bare hostname. Two commands
/// choosing an address by different rules would report two different machines.
pub(super) const OFFLINE_SNIPPET: &str = r##"sh <<'STADO_OFFLINE_INVITE'
# stado offline invite for '@TARGET@' -- run this ON THE MACHINE BEING ADDED.
#
# Nothing in this text is a secret. The only key in it is the fleet's PUBLIC
# half; the private half never leaves the operator's vault, so whoever reads
# this fragment gains no access to anything, here or anywhere else.
#
# What it does, and nothing more:
#   1. creates ~/.ssh (mode 700) and ~/.ssh/authorized_keys (mode 600),
#   2. appends the fleet's public key there, once, even if it runs twice,
#   3. checks whether an SSH server answers on port 22 and, if not, prints how
#      to turn one on -- it never turns anything on itself,
#   4. prints the user@address to send back to the operator.
# It installs no software, starts no service, and generates no key.
set -eu

fleet_key='@FLEET_KEY@'
fleet_target='@TARGET@'

say() {
    printf '%s\n' "$*"
}

die() {
    printf '%s\n' "$*" >&2
    exit 1
}

for required in awk mkdir chmod tail uname id; do
    command -v "$required" >/dev/null 2>&1 ||
        die "this machine has no $required, which this fragment needs"
done

key_type="$(printf '%s' "$fleet_key" | awk '{ print $1 }')"
key_blob="$(printf '%s' "$fleet_key" | awk '{ print $2 }')"
[ -n "$key_blob" ] || die 'the pasted fragment carries no key material'

# ------------------------------------------------------------- install the key

ssh_dir="$HOME/.ssh"
authorized_keys="$ssh_dir/authorized_keys"

[ -d "$ssh_dir" ] || mkdir -p "$ssh_dir"
chmod 700 "$ssh_dir"
[ -f "$authorized_keys" ] || (umask 077; : >"$authorized_keys")
chmod 600 "$authorized_keys"

# Idempotent on the key material, not on the whole line: a re-issued invite may
# carry a different comment, and a second run must not leave the same key twice.
if awk -v type="$key_type" -v blob="$key_blob" '
        $1 == type && $2 == blob { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$authorized_keys"; then
    key_action='already present'
else
    # Append on its own line even when the file did not end in a newline.
    if [ -s "$authorized_keys" ] && [ -n "$(tail -c 1 "$authorized_keys")" ]; then
        printf '\n' >>"$authorized_keys"
    fi
    printf '%s\n' "$fleet_key" >>"$authorized_keys"
    key_action='installed'
fi

# ------------------------------------------------------------- reachability

os_name="$(uname -s)"
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
        awk 'match($0, /"DNSName"[ \t]*:[ \t]*"[^"]*"/) {
                field = substr($0, RSTART, RLENGTH)
                sub(/^"DNSName"[ \t]*:[ \t]*"/, "", field)
                sub(/"$/, "", field)
                print field
                exit
            }' || true)"
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

# ------------------------------------------------------------- sshd probe

# The fleet dials in over SSH, so a machine with no SSH server answering is
# reachable in name only. Remote Login is the owner's decision and needs
# administrator rights: diagnose it, print the exact way to turn it on, and
# never turn it on here.
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
Turn on Remote Login yourself -- it needs administrator rights, so this
fragment will not do it for you:

  System Settings > General > Sharing > Remote Login  (switch it on, and under
  the (i) button allow access for your own user)

The equivalent from a terminal, which will ask for your password:

  sudo systemsetup -setremotelogin on
MACOS
            ;;
        Linux)
            cat <<'LINUX'
Start an SSH server yourself -- it needs root, so this fragment will not do it
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
fragment will not start one for you.
OTHER
            ;;
    esac
}

# ------------------------------------------------------------- summary

say ''
say '--------------------------------------------------------------'
say "Stado offline invite for '$fleet_target'"
say '--------------------------------------------------------------'
say "  Fleet key ($key_type): $key_action in $authorized_keys"
say '  Nothing was installed here and no service was started.'
say '  This fragment held only a PUBLIC key: no private key was received,'
say '  generated or sent anywhere by it.'
say ''
case "$ssh_listening" in
    yes)
        say 'Remote login: an SSH server is answering on port 22.'
        ;;
    no)
        say 'Remote login: NOTHING is answering on port 22, so the fleet cannot'
        say 'reach this machine yet. Turn it on before the operator tries:'
        say ''
        ssh_instructions
        ;;
    *)
        say 'Remote login: could not be checked here (no nc, no ssh client). The'
        say 'fleet needs an SSH server answering on port 22:'
        say ''
        ssh_instructions
        ;;
esac
say ''
say "Send this line back to the operator (chosen as the $address_kind):"
say "$login_user@$address"
STADO_OFFLINE_INVITE
"##;
