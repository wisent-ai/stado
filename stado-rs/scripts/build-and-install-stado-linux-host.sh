#!/usr/bin/env bash
# Build `stado` from origin/main on this Linux host and put it behind the gate.
#
# The fleet's only Linux machine is also its only GPU machine, and a fix to how
# the agent measures accelerators is invisible until this host's binary moves.
# `install_rust_toolchain.sh` is the reason it can be built here at all: a cross
# build from macOS fails in `ring`, and an emulated container build ran over an
# hour.
#
# The same discipline `install-built-stado-binary.py` applies on the Mac:
#
#   - the new binary must report a version that is not older than the installed
#     one;
#   - it must answer two read-only control-plane questions the way the installed
#     binary does, so a build that cannot see the fleet never replaces one that
#     can;
#   - the previous binary is kept beside the new one under its version and the
#     date, so one `cp` puts it back.
#
# The agent unit is restarted afterwards, because the point of the swap is the
# agent, and its capacity broadcast is the evidence. Takes no operator words: a
# helper that took them would be a remote shell.
set -euo pipefail

REPO=https://github.com/wisent-ai/stado.git
WORK=/root/.stado/build-work/stado
BIN=/root/.stado/bin/stado
CARGO_HOME_DIR=/root/.cargo
export PATH="$CARGO_HOME_DIR/bin:$PATH"
export CARGO_TARGET_DIR=/root/.cache/stado-build

command -v cargo >/dev/null 2>&1 || {
  printf 'ERROR\tcargo absent; install the toolchain first (install-rust-toolchain helper)\n' >&2
  exit 1
}

mkdir -p "$(dirname "$WORK")" "$CARGO_TARGET_DIR"
if [ -d "$WORK/.git" ]; then
  git -C "$WORK" remote set-url origin "$REPO"
  git -C "$WORK" fetch --quiet origin main
  git -C "$WORK" reset --hard --quiet origin/main
else
  rm -rf "$WORK"
  git clone --quiet --depth 50 --branch main "$REPO" "$WORK"
fi
printf 'SOURCE\t%s\n' "$(git -C "$WORK" rev-parse --short HEAD)"

# `wisent-errors` is a private git dependency and this host holds no credential
# for it, so cargo cannot clone it and the build stops before it starts. The
# crate is delivered as a vendored directory instead (`stado host install-file
# wisent-errors-vendor.tgz`), and only that one source is replaced: crates.io
# stays live, so nothing else is pinned to a stale copy. Same revision either
# way -- the vendor tree carries cargo's own checksum file.
VENDOR_TGZ=/root/.stado/files/wisent-errors-vendor.tgz
VENDOR_DIR=/root/.stado/build-work/wisent-errors-vendor
if [ -f "$VENDOR_TGZ" ]; then
  rm -rf "$VENDOR_DIR"
  mkdir -p "$VENDOR_DIR"
  tar xzf "$VENDOR_TGZ" -C "$VENDOR_DIR"
  rev=$(awk -F'"' '/^wisent-errors = / { for (i = 1; i <= NF; i++) if ($i ~ /^[0-9a-f]{40}$/) print $i }' \
    "$WORK/stado-rs/Cargo.toml" | head -n 1)
  [ -n "$rev" ] || { printf 'ERROR\tcannot read the wisent-errors revision from Cargo.toml\n' >&2; exit 1; }
  mkdir -p "$WORK/stado-rs/.cargo"
  cat >"$WORK/stado-rs/.cargo/config.toml" <<EOF
[source."git+https://github.com/wisent-ai/wisent-errors?rev=$rev"]
git = "https://github.com/wisent-ai/wisent-errors"
rev = "$rev"
replace-with = "wisent-errors-vendor"

[source.wisent-errors-vendor]
directory = "$VENDOR_DIR"
EOF
  printf 'VENDOR\twisent-errors %s replaced from %s\n' "$rev" "$VENDOR_DIR"
else
  printf 'VENDOR\tabsent; cargo will try to clone the private dependency\n'
fi
printf 'BUILD\tstarting release build\n'
( cd "$WORK/stado-rs" && cargo build --release --quiet --bin stado )
CANDIDATE="$CARGO_TARGET_DIR/release/stado"
[ -x "$CANDIDATE" ] || { printf 'ERROR\tno artifact at %s\n' "$CANDIDATE" >&2; exit 1; }

new_version=$("$CANDIDATE" --version | awk '{print $NF}')
old_version=$("$BIN" --version 2>/dev/null | awk '{print $NF}' || echo 0.0.0)
printf 'VERSION\tinstalled %s -> candidate %s\n' "$old_version" "$new_version"

# Refuse a version that goes backwards. Sort -V puts the older first; if the
# candidate sorts first and differs, it is older.
older=$(printf '%s\n%s\n' "$new_version" "$old_version" | sort -V | head -n 1)
if [ "$older" = "$new_version" ] && [ "$new_version" != "$old_version" ]; then
  printf 'ERROR\tcandidate %s is older than installed %s\n' "$new_version" "$old_version" >&2
  exit 1
fi

# Two read-only control-plane answers must agree. Where the OLD binary fails and
# the new one answers, that is a repair, not a disagreement.
for probe in "registry self" "registry pull"; do
  # shellcheck disable=SC2086
  if old_out=$("$BIN" $probe 2>/dev/null); then old_ok=1; else old_ok=0; old_out=""; fi
  # shellcheck disable=SC2086
  if new_out=$("$CANDIDATE" $probe 2>/dev/null); then new_ok=1; else new_ok=0; new_out=""; fi
  if [ "$new_ok" -eq 0 ]; then
    printf 'ERROR\tcandidate cannot answer `%s`; refusing the swap\n' "$probe" >&2
    exit 1
  fi
  if [ "$old_ok" -eq 1 ]; then
    old_sum=$(printf '%s' "$old_out" | sha256sum | cut -d' ' -f1)
    new_sum=$(printf '%s' "$new_out" | sha256sum | cut -d' ' -f1)
    if [ "$old_sum" = "$new_sum" ]; then
      printf 'PROBE\t%s\tidentical\n' "$probe"
    else
      printf 'ERROR\t%s differs between installed and candidate\n' "$probe" >&2
      exit 1
    fi
  else
    printf 'PROBE\t%s\trepair (installed binary could not answer)\n' "$probe"
  fi
done

backup="$BIN.$old_version-backup-$(date -u +%Y%m%d)"
cp -p "$BIN" "$backup"
install -m 0700 "$CANDIDATE" "$BIN"
printf 'INSTALL\t%s (previous kept at %s)\n' "$("$BIN" --version)" "$backup"

systemctl restart wisent-agent.service
sleep 15
printf 'UNIT\t%s\n' "$(systemctl is-active wisent-agent.service)"

printf '\nAGENT_LOG_TAIL\n'
journalctl -u wisent-agent.service --no-pager -n 12 -o cat | tail -n 12
