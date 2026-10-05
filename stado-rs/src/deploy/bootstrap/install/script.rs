//! Stage one, first half: the remote release-download script, plus the two
//! installed-path constants the parsed script output resolves against.

use crate::deploy::shlex_quote;

/// The installer, fed to a remote host as the remote command argument (not
/// stdin): the repository's own `install-stado.sh`, so the file a person runs
/// and the script `stado bootstrap` runs are one. It downloads the signed
/// release `stado release submit` publishes — `release.json` and
/// `release.tar.gz` — over HTTPS, checks the manifest's identity, digest and
/// size against the archive, then prints the platform and the installed Stado
/// path as the final two stdout lines. Verification is POSIX tools only: a
/// host being bootstrapped has no Stado yet and needs no interpreter.
pub const REMOTE_INSTALL_SCRIPT: &str = include_str!("../../../../../install-stado.sh");

/// [`REMOTE_INSTALL_SCRIPT`] with the immutable release coordinates bound in.
/// Both values are shell-quoted and validated again by the script.
pub fn remote_install_script(api_url: &str, version: &str) -> String {
    format!(
        "export STADO_API_URL={}\nexport STADO_RELEASE_VERSION={}\n{REMOTE_INSTALL_SCRIPT}",
        shlex_quote(api_url),
        shlex_quote(version)
    )
}

/// Default stado path used when the remote install prints nothing, and
/// as the dry-run placeholder.
pub const WC_BIN_DEFAULT: &str = "$HOME/.stado/bin/stado";
