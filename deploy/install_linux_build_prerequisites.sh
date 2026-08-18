#!/usr/bin/env bash
# Ensure the Linux builder can compile the fleet's TLS-using crates.
#
# brama's release build reached `openssl-sys 0.9.113` and stopped: "Make sure you
# also have the development packages of openssl installed. For example,
# `libssl-dev` on Ubuntu." The quality gate had already passed and both the source
# and its pinned Skarbiec had compiled, so the build looked healthy until the first
# crate that links against a system library.
#
# This is the third prerequisite this host turned out to be missing, after the
# rustfmt/clippy components and a git credential, and each was invisible until a
# build reached it. Ensuring them from a checked-in script means the next builder
# is one command away from ready instead of one failed release away.
#
# Idempotent: a package already present is reported and left alone.
set -euo pipefail

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'user %s\n' "$(id -un)"

if ! command -v apt-get >/dev/null; then
  printf 'apt-get unavailable; this script targets the Debian-family builder\n' >&2
  exit 64
fi

# The helper already runs as root on this host, so no privilege escalation is
# needed or attempted; a non-root run says so rather than prompting for anything.
if [ "$(id -u)" != "0" ]; then
  printf 'must run as root to install packages; refusing to prompt\n' >&2
  exit 65
fi

export DEBIAN_FRONTEND=noninteractive
missing=""
for package in pkg-config libssl-dev; do
  if dpkg-query -W -f='${Status}' "$package" 2>/dev/null | grep -q 'install ok installed'; then
    printf 'package present %s\n' "$package"
  else
    printf 'package missing %s\n' "$package"
    missing="$missing $package"
  fi
done

if [ -n "$missing" ]; then
  apt-get update -qq
  # shellcheck disable=SC2086
  apt-get install -y -qq $missing >/dev/null
  for package in $missing; do
    if dpkg-query -W -f='${Status}' "$package" 2>/dev/null | grep -q 'install ok installed'; then
      printf 'package installed %s\n' "$package"
    else
      printf 'package FAILED %s\n' "$package" >&2
      exit 66
    fi
  done
fi

# Prove what the build actually needs: the linker flags, not the package list.
if command -v pkg-config >/dev/null && pkg-config --exists openssl; then
  printf 'openssl discoverable version=%s\n' "$(pkg-config --modversion openssl)"
else
  printf 'openssl still not discoverable by pkg-config\n' >&2
  exit 67
fi
