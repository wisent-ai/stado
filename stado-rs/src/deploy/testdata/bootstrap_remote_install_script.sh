set -euo pipefail
BIN_DIR="$HOME/.stado/bin"
mkdir -p "$BIN_DIR"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-amd64 ;;
  Darwin-arm64) platform=darwin-arm64 ;;
  *) echo "unsupported platform: $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
case "$release_api" in
  https://*) ;;
  *) echo "STADO_RELEASE_API_URL must use HTTPS"; false ;;
esac
case "$release_version" in
  *[![:alnum:]._-]*|"") echo "invalid STADO_RELEASE_VERSION"; false ;;
esac
release_api="${release_api%/}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
release_get() {
  curl -fsSL --get \
    --data-urlencode "uri=stado://releases/stado/$release_version/$platform/$name" \
    "$release_api/api/release/object" \
    -o "$tmp/$name"
}
for name in stado stado-fix stado-watchdog SHA256SUMS; do
  release_get
done
verify() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum -c -; else shasum -a 256 -c -; fi
}
(cd "$tmp" && for name in stado stado-fix stado-watchdog; do grep -E "[ *]$name\$" SHA256SUMS | verify; done)
for name in stado stado-fix stado-watchdog; do
  chmod 755 "$tmp/$name"
  mv "$tmp/$name" "$BIN_DIR/$name"
done
echo "$platform"
python3 -c 'import sys; sys.stdout.write(sys.executable + "\n")'
echo "$BIN_DIR/stado"
