#!/bin/sh
set -eu
cargo_bin=
for candidate in \
  "$HOME/.cargo/bin/cargo" \
  "$HOME"/.rustup/toolchains/*/bin/cargo \
  /usr/local/cargo/bin/cargo \
  /usr/local/bin/cargo \
  /usr/bin/cargo
do
  if [ -x "$candidate" ]; then
    cargo_bin="$candidate"
    break
  fi
done
[ -n "$cargo_bin" ] || { printf '%s\n' "no installed Rust toolchain found" >&2; exit 1; }

archive="$HOME/.stado/stado-native-source.tar.gz"
work="$HOME/.stado/build-native-stado"
next="$HOME/.stado/bin/stado.next"

[ -f "$archive" ] || { printf '%s\n' "missing $archive" >&2; exit 1; }
rm -rf "$work"
mkdir -p "$work" "$HOME/.stado/bin"
tar -xzf "$archive" -C "$work"
"$cargo_bin" build --release --locked --manifest-path "$work/stado-rs/Cargo.toml" --bin stado
install -m 0755 "$work/stado-rs/target/release/stado" "$next"
"$next" --version
mv -f "$next" "$HOME/.stado/bin/stado"
printf '%s\n' "installed $HOME/.stado/bin/stado"
