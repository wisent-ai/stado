#!/usr/bin/env bash
# Start (or report) a detached release build of `stado` on this Linux host.
#
# A from-scratch release build of this crate ran past an hour here, and
# `stado host run-helper` waits on its ssh channel: when the caller's timeout
# fired, the build died with the channel and left a warm target directory and no
# artifact. So the build is detached from the channel and this helper is
# idempotent -- called again it reports the running build's progress, or the
# finished build's verdict, instead of starting a second one.
#
# Pair it with `install-built-stado-linux-host.sh`, which owns the gated swap.
# Takes no operator words: a helper that took them would be a remote shell.
set -euo pipefail

REPO=https://github.com/wisent-ai/stado.git
WORK=/root/.stado/build-work/stado
LOG=/root/.stado/build-work/stado-build.log
PIDFILE=/root/.stado/build-work/stado-build.pid
VENDOR_TGZ=/root/.stado/files/wisent-errors-vendor.tgz
VENDOR_DIR=/root/.stado/build-work/wisent-errors-vendor
export PATH="/root/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/root/.cache/stado-build
CANDIDATE="$CARGO_TARGET_DIR/release/stado"

if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
  printf 'STATE\trunning (pid %s)\n' "$(cat "$PIDFILE")"
  printf 'ELAPSED\t%s seconds\n' "$(( $(date +%s) - $(stat -c %Y "$PIDFILE") ))"
  printf 'LOG_TAIL\n'
  tail -n 15 "$LOG" 2>/dev/null || true
  exit 0
fi

if [ -x "$CANDIDATE" ] && [ -f "$LOG" ] && grep -q '^BUILD_EXIT 0$' "$LOG"; then
  printf 'STATE\tcomplete\n'
  printf 'ARTIFACT\t%s\t%s\n' "$CANDIDATE" "$("$CANDIDATE" --version 2>&1)"
  printf 'SOURCE\t%s\n' "$(git -C "$WORK" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  exit 0
fi

command -v cargo >/dev/null 2>&1 || {
  printf 'ERROR\tcargo absent; run the install-rust-toolchain helper first\n' >&2
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

# The private dependency, vendored: this host holds no credential for it, so
# cargo cannot clone it. Only that one source is replaced; crates.io stays live.
if [ -f "$VENDOR_TGZ" ]; then
  rm -rf "$VENDOR_DIR"
  mkdir -p "$VENDOR_DIR"
  tar xzf "$VENDOR_TGZ" -C "$VENDOR_DIR"
  rev=$(awk -F'"' '/^wisent-errors = / { for (i = 1; i <= NF; i++) if ($i ~ /^[0-9a-f]{40}$/) print $i }' \
    "$WORK/stado-rs/Cargo.toml" | head -n 1)
  [ -n "$rev" ] || { printf 'ERROR\tcannot read the wisent-errors revision\n' >&2; exit 1; }
  mkdir -p "$WORK/stado-rs/.cargo"
  cat >"$WORK/stado-rs/.cargo/config.toml" <<EOF
[source."git+https://github.com/wisent-ai/wisent-errors?rev=$rev"]
git = "https://github.com/wisent-ai/wisent-errors"
rev = "$rev"
replace-with = "wisent-errors-vendor"

[source.wisent-errors-vendor]
directory = "$VENDOR_DIR"
EOF
  printf 'VENDOR\twisent-errors %s from %s\n' "$rev" "$VENDOR_DIR"
else
  printf 'VENDOR\tabsent; cargo will try to clone the private dependency\n'
fi

: >"$LOG"
setsid nohup bash -c "
  cd '$WORK/stado-rs'
  PATH=/root/.cargo/bin:\$PATH CARGO_TARGET_DIR='$CARGO_TARGET_DIR' cargo build --release --bin stado >>'$LOG' 2>&1
  printf 'BUILD_EXIT %s\n' \"\$?\" >>'$LOG'
  rm -f '$PIDFILE'
" >/dev/null 2>&1 &
echo $! >"$PIDFILE"
printf 'STATE\tstarted (pid %s); log %s\n' "$(cat "$PIDFILE")" "$LOG"
