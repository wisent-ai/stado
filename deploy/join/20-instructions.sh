
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
