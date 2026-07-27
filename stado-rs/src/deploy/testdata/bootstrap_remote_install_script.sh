set -euo pipefail
BIN_DIR="$HOME/.stado/bin"
mkdir -p "$BIN_DIR"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-amd64 ;;
  Darwin-arm64) platform=darwin-arm64 ;;
  *) echo "unsupported platform: $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
release_qs=""
case "$release_base" in
  *\?*)
    release_qs="${release_base#*\?}"
    release_base="${release_base%%\?*}"
    ;;
esac
release_base="${release_base%/}"
latest="$(curl -fsSL "$release_base/latest.json${release_qs:+?$release_qs}")"
version="${latest#*\"version\": \"}"
version="${version%%\"*}"
prefix="$release_base/$version/$platform"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cache_bust="$(date +%s)"
for name in stado wc stado-fix stado-watchdog SHA256SUMS; do
  curl -fsSL "$prefix/$name?cache_bust=$cache_bust${release_qs:+&$release_qs}" -o "$tmp/$name"
done
verify() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum -c -; else shasum -a 256 -c -; fi
}
(cd "$tmp" && for name in stado wc stado-fix stado-watchdog; do grep -E "[ *]$name\$" SHA256SUMS | verify; done)
for name in stado wc stado-fix stado-watchdog; do
  chmod 755 "$tmp/$name"
  mv "$tmp/$name" "$BIN_DIR/$name"
done
echo "$platform"
python3 -c 'import sys; sys.stdout.write(sys.executable + "\n")'
echo "$BIN_DIR/stado"
