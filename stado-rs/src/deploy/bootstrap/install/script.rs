//! Stage one, first half: the remote release-download script, plus the two
//! installed-path constants the parsed script output resolves against.

use crate::deploy::shlex_quote;

/// Remote install script BODY (fed as the remote command argument, not
/// stdin). Downloads release artifacts over HTTPS, checksum-verifies them,
/// then prints the platform, the job-runtime Python path and the installed
/// Stado path as the final three stdout lines. Public HTTPS keeps bootstrap
/// independent of any cloud CLI or object-store locator.
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
python3 - "$tmp" "$release_version" "$platform" <<'PY'
import hashlib, json, os, pathlib, sys, tarfile
root, version, platform = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
manifest = json.loads((root / f"release-manifest-{platform}.json").read_text())
if set(manifest) != {"product", "version", "platform", "source_commit", "sha256"}:
    raise SystemExit("release manifest has unexpected fields")
if (manifest["product"], manifest["version"], manifest["platform"]) != ("stado", version, platform):
    raise SystemExit("release manifest identity mismatch")
if not isinstance(manifest["source_commit"], str) or len(manifest["source_commit"]) not in (40, 64):
    raise SystemExit("release manifest source commit is invalid")
if any(character not in "0123456789abcdefABCDEF" for character in manifest["source_commit"]):
    raise SystemExit("release manifest source commit is invalid")
if not isinstance(manifest["sha256"], str) or len(manifest["sha256"]) != 64:
    raise SystemExit("release manifest digest is invalid")
if any(character not in "0123456789abcdef" for character in manifest["sha256"]):
    raise SystemExit("release manifest digest is invalid")
archive = root / f"stado-v{version}-{platform}.tar.gz"
if hashlib.sha256(archive.read_bytes()).hexdigest() != manifest["sha256"]:
    raise SystemExit("release archive digest mismatch")
required = {"stado", "stado-fix", "stado-watchdog"}
with tarfile.open(archive, "r:gz") as bundle:
    members = bundle.getmembers()
    for name in required:
        matches = [member for member in members if member.name == name and member.isfile()]
        if len(matches) != 1:
            raise SystemExit(f"release archive has invalid member {name}")
        source = bundle.extractfile(matches[0])
        if source is None:
            raise SystemExit(f"release archive cannot read member {name}")
        destination = root / name
        destination.write_bytes(source.read())
        os.chmod(destination, 0o755)
PY
for name in stado stado-fix stado-watchdog; do
  mv "$tmp/$name" "$BIN_DIR/$name"
done
echo "$platform"
python3 -c 'import sys; sys.stdout.write(sys.executable + "\n")'
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

/// Default WC_PYTHON used when the remote install prints no python path,
/// and as the dry-run placeholder. Matches the agent's own default
/// (`providers::local::python_bin`).
pub const WC_PYTHON_DEFAULT: &str = "python3";
