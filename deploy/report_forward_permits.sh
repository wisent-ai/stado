#!/usr/bin/env bash
# Report which forwarding destinations this host allows over ssh.
#
# The resolver reaches a loopback service on another host with `ssh -W`, and
# the peer refuses the channel when the destination is outside the key's
# permitopen list. That refusal reads as "Session open refused by peer" on the
# calling side and says nothing about which destinations are allowed, so the
# difference between "service is down" and "this port was never permitted" is
# invisible from there.
#
# Read-only. Prints the permitopen options and the sshd directives, never a
# key body.
set -u

echo "=== permitopen options in authorized_keys ==="
if [ -r "$HOME/.ssh/authorized_keys" ]; then
  tr ' ' '\n' < "$HOME/.ssh/authorized_keys" | sed -n '/permitopen/p' | sort -u
else
  echo "(no readable ~/.ssh/authorized_keys)"
fi

echo
echo "=== PermitOpen in sshd configuration ==="
for f in /etc/ssh/sshd_config /etc/ssh/sshd_config.d/*; do
  [ -r "$f" ] || continue
  sed -n '/^[[:space:]]*PermitOpen/p' "$f" | sed "s|^|$f: |"
done
echo "(empty above means sshd imposes no PermitOpen of its own)"
