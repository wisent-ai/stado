//! Locating the installed `skarbiec` binary an owner write runs through.

use std::path::PathBuf;

use crate::skarbiec::SkarbiecError;

/// Where Stado installs Skarbiec, mirroring
/// [`crate::deploy::host_recovery::WC_CANDIDATES`]: one prefix, discovered the
/// same way, so the two cannot drift apart.
const SKARBIEC_CANDIDATES: &[&str] = &["$HOME/.stado/bin/skarbiec"];

pub(super) fn home() -> Result<String, SkarbiecError> {
    std::env::var("HOME").map_err(|_| SkarbiecError::Deployment("HOME is not set".to_string()))
}

/// Resolve the installed `skarbiec` binary.
///
/// `SKARBIEC_BIN` is the override the credential scripts already use, and it is
/// the only way to exercise a build before it is installed — which is the
/// situation whenever the installed binary is the thing that is stale.
pub fn binary() -> Result<PathBuf, SkarbiecError> {
    if let Ok(explicit) = std::env::var("SKARBIEC_BIN") {
        let path = PathBuf::from(&explicit);
        if !path.is_file() {
            return Err(SkarbiecError::Deployment(format!(
                "SKARBIEC_BIN names no file: {explicit}"
            )));
        }
        return Ok(path);
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
