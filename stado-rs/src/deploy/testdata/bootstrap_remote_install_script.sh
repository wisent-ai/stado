set -euo pipefail
BIN_DIR="$HOME/.stado/bin"
mkdir -p "$BIN_DIR"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-amd64 ;;
  Darwin-arm64) platform=darwin-arm64 ;;
  *) echo "unsupported platform: $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
version="$(gcloud storage cp gs://wisent-compute/releases/stado/latest.json - | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["version"])')"
prefix="gs://wisent-compute/releases/stado/$version/$platform"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for name in stado wc stado-watchdog SHA256SUMS; do
  gcloud storage cp "$prefix/$name" "$tmp/$name"
done
verify() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum -c -; else shasum -a 256 -c -; fi
}
(cd "$tmp" && for name in stado wc stado-watchdog; do grep -E "[ *]$name\$" SHA256SUMS | verify; done)
for name in stado wc stado-watchdog; do
  chmod 755 "$tmp/$name"
  mv "$tmp/$name" "$BIN_DIR/$name"
done
python3 -c 'import sys; sys.stdout.write(sys.executable + "\n")'
echo "$BIN_DIR/stado"
