//! Locating the installed `skarbiec` binary an owner write runs through.

use std::path::PathBuf;

use crate::skarbiec::SkarbiecError;

/// Where Stado installs Skarbiec, mirroring
/// [`crate::deploy::host_recovery::WC_CANDIDATES`]: one prefix, discovered the
/// same way, so the two cannot drift apart.
const SKARBIEC_CANDIDATES: &[&str] = &["$HOME/.stado/bin/skarbiec"];

/// The home the vault and binary candidates sit under, as text. A home that
/// is not text is refused naming it; read with `std::env::var`, it was
/// reported as unset.
pub(super) fn home() -> Result<String, SkarbiecError> {
    crate::config_file::home()
        .map_err(SkarbiecError::Deployment)?
        .into_os_string()
        .into_string()
        .map_err(|home| {
            SkarbiecError::Deployment(format!(
                "the home directory {home:?} is not text, so no vault or skarbiec path can be named under it"
            ))
        })
}

/// Resolve the installed `skarbiec` binary.
///
/// `SKARBIEC_BIN` is the override the credential scripts already use, and it is
/// the only way to exercise a build before it is installed — which is the
/// situation whenever the installed binary is the thing that is stale. A
/// value that is not text is refused rather than passed over for the
/// installed binary.
pub fn binary() -> Result<PathBuf, SkarbiecError> {
    match std::env::var("SKARBIEC_BIN") {
        Ok(explicit) => {
            let path = PathBuf::from(&explicit);
            if !path.is_file() {
                return Err(SkarbiecError::Deployment(format!(
                    "SKARBIEC_BIN names no file: {explicit}"
                )));
            }
            return Ok(path);
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(error) => {
            return Err(SkarbiecError::Deployment(format!(
                "SKARBIEC_BIN cannot be read: {error}"
            )))
        }
    }
    let home = home()?;
    for candidate in SKARBIEC_CANDIDATES {
        let path = PathBuf::from(candidate.replace("$HOME", &home));
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(SkarbiecError::Deployment(format!(
        "no installed skarbiec binary at {}",
        SKARBIEC_CANDIDATES.join(", ")
    )))
}
