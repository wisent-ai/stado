#!/usr/bin/env bash
# Report what is using the root filesystem on this host.
#
# The beacon says the disk is nearly full; it cannot say what filled it, and
# the registry cleanup planner refuses to run here because the host has no
# stado binary. Knowing which directories hold the space is what decides
# whether the answer is a policy the janitor already has, or a human decision
# about data on a compute machine.
#
# Read-only. Prints sizes and paths, touches nothing. Permission errors are
# left visible rather than hidden: a directory this cannot read is one the
# report is silent about, and silence is what made the disk a surprise.
set -u

echo "=== filesystem ==="
df -h /

echo
echo "=== largest top-level directories ==="
du -shx /* | sort -rh | head

echo
echo "=== inside /var ==="
du -shx /var/* | sort -rh | head

echo
echo "=== inside the home of the account that runs stado ==="
du -shx "${HOME:-/root}"/* | sort -rh | head
