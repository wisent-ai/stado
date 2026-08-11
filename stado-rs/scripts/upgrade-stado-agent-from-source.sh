#!/bin/sh
set -eu

repo="$HOME/.stado/build/stado"
/bin/mkdir -p "$HOME/.stado/build" "$HOME/.stado/bin"
if [ -d "$repo/.git" ]; then
  /usr/bin/git -C "$repo" fetch --prune origin main
else
  /usr/bin/git clone --filter=blob:none https://github.com/wisent-ai/stado.git "$repo"
fi
/usr/bin/git -C "$repo" checkout --detach origin/main
cargo_bin=$(command -v cargo || true)
[ -n "$cargo_bin" ] || cargo_bin="$HOME/.cargo/bin/cargo"
[ -x "$cargo_bin" ]
"$cargo_bin" build --release --manifest-path "$repo/stado-rs/Cargo.toml" --bin stado
/usr/bin/install -m 0755 "$repo/stado-rs/target/release/stado" "$HOME/.stado/bin/stado.next"
/bin/mv "$HOME/.stado/bin/stado.next" "$HOME/.stado/bin/stado"
"$HOME/.stado/bin/stado" --version
