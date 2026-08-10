#!/bin/sh
set -eu
PATH="$HOME/.cargo/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin"
export PATH

archive="$HOME/.stado/stado-native-source.tar.gz"
work="$HOME/.stado/build-native-stado"
next="$HOME/.stado/bin/stado.next"

[ -f "$archive" ] || { printf '%s\n' "missing $archive" >&2; exit 1; }
rm -rf "$work"
mkdir -p "$work" "$HOME/.stado/bin"
tar -xzf "$archive" -C "$work"
cargo build --release --locked --manifest-path "$work/stado-rs/Cargo.toml" --bin stado
install -m 0755 "$work/stado-rs/target/release/stado" "$next"
"$next" --version
mv -f "$next" "$HOME/.stado/bin/stado"
printf '%s\n' "installed $HOME/.stado/bin/stado"
