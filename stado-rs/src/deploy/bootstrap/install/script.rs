//! Stage one, first half: the remote release-download script, plus the two
//! installed-path constants the parsed script output resolves against.

use crate::deploy::shlex_quote;

/// Remote install script BODY (fed as the remote command argument, not
/// stdin). Downloads release artifacts over HTTPS, checksum-verifies them,
/// then prints the platform and the installed Stado path as the final two
/// stdout lines. Public HTTPS keeps bootstrap independent of any cloud CLI
/// or object-store locator. Verification is POSIX tools only: a host being
/// bootstrapped has no Stado yet and needs no interpreter.
///
/// [`remote_install_script`] binds the exact version and public Stado API
/// origin. The remote consumes only canonical `stado://releases/...` objects
/// through `/api/release/object`; it never discovers a channel pointer.
pub const REMOTE_INSTALL_SCRIPT: &str = r#"set -euo pipefail
BIN_DIR="$HOME/.stado/bin"
mkdir -p "$BIN_DIR"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-amd64 ;;
  Darwin-arm64) platform=darwin-arm64 ;;
  *) echo "unsupported platform: $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
case "$release_api" in
  https://*) ;;
  *) echo "STADO_API_URL must use HTTPS"; false ;;
esac
case "$release_version" in
  *[![:alnum:]._-]*|"") echo "invalid STADO_RELEASE_VERSION"; false ;;
esac
release_api="${release_api%/}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
manifest_name="release-manifest-$platform.json"
archive_name="stado-v$release_version-$platform.tar.gz"
for name in "$manifest_name" "$archive_name"; do
  curl -fsSL --get \
    --data-urlencode "uri=stado://releases/stado/$release_version/$platform/$name" \
    "$release_api/api/release/object" \
    -o "$tmp/$name"
done
# One JSON string field of the manifest, or nothing.
field() {
  tr -d '\n\r' < "$tmp/$manifest_name" \
    | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p"
}
fail() { echo "$1" >&2; exit 1; }
keys=$(tr -d '\n\r' < "$tmp/$manifest_name" | grep -o '"[a-z_]*"[[:space:]]*:' | tr -d ' :"' | sort | tr '\n' ' ')
[ "$keys" = "platform product sha256 source_commit version " ] || fail "release manifest has unexpected fields"
[ "$(field product)" = stado ] && [ "$(field version)" = "$release_version" ] && [ "$(field platform)" = "$platform" ] \
  || fail "release manifest identity mismatch"
commit=$(field source_commit)
case "$commit" in *[!0-9a-fA-F]*|"") fail "release manifest source commit is invalid" ;; esac
[ "${#commit}" = 40 ] || [ "${#commit}" = 64 ] || fail "release manifest source commit is invalid"
digest=$(field sha256)
case "$digest" in *[!0-9a-f]*|"") fail "release manifest digest is invalid" ;; esac
[ "${#digest}" = 64 ] || fail "release manifest digest is invalid"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$archive_name" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$tmp/$archive_name" | cut -d' ' -f1)
fi
[ "$actual" = "$digest" ] || fail "release archive digest mismatch"
mkdir "$tmp/out"
for name in stado stado-fix stado-watchdog; do
  [ "$(tar -tzf "$tmp/$archive_name" | grep -cx "$name")" = 1 ] || fail "release archive has invalid member $name"
done
tar -xzf "$tmp/$archive_name" -C "$tmp/out" stado stado-fix stado-watchdog
for name in stado stado-fix stado-watchdog; do
  { [ -f "$tmp/out/$name" ] && [ ! -L "$tmp/out/$name" ]; } || fail "release archive has invalid member $name"
  chmod 755 "$tmp/out/$name"
  mv "$tmp/out/$name" "$BIN_DIR/$name"
done
echo "$platform"
echo "$BIN_DIR/stado"
"#;

/// [`REMOTE_INSTALL_SCRIPT`] with the immutable release coordinates bound in.
/// Both values are shell-quoted and validated again by the remote script.
pub fn remote_install_script(api_url: &str, version: &str) -> String {
    format!(
        "release_api={}\nrelease_version={}\n{REMOTE_INSTALL_SCRIPT}",
        shlex_quote(api_url),
        shlex_quote(version)
    )
}

/// Default stado path used when the remote install prints nothing, and
/// as the dry-run placeholder.
pub const WC_BIN_DEFAULT: &str = "$HOME/.stado/bin/stado";

