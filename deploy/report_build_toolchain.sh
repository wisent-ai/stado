#!/usr/bin/env bash
# Report whether this host can build a Stado binary for itself.
#
# The fleet has one Linux machine, and it is also the only place a linux
# binary for it can be produced without a cross toolchain. Whether that is
# possible decides whether a missing component is a deployment or a project.
#
# Read-only. Prints tool versions, nothing else.
set -eu

for tool in cargo rustc git; do
  if command -v "$tool" >/dev/null; then
    printf 'present  %s  %s\n' "$tool" "$("$tool" --version 2>&1 | head -n 1)"
  else
    printf 'MISSING  %s\n' "$tool"
  fi
done

printf 'arch     %s\n' "$(uname -m)"
printf 'kernel   %s\n' "$(uname -s)"
