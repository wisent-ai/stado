//! Binaries: where the two programs an entrance is made of are found, and what
//! a miss is allowed to say.

use std::path::{Path, PathBuf};

use crate::cli::fleet::ingress::CLOUDFLARED_CANDIDATES;

/// Resolve `cloudflared` the way [`crate::credential_store::owner::binary`]
/// resolves Skarbiec: an explicit environment override first, then the known
/// install prefixes, then `PATH`.
///
/// The refusal names every place that was looked in, because the fix is
/// different for each miss — the tool is not installed, or it is installed
/// somewhere this list does not know, and an operator cannot tell those apart
/// from "cloudflared not found".
pub fn cloudflared_binary() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("STADO_CLOUDFLARED_BIN") {
        let path = PathBuf::from(explicit.trim());
        if !path.is_file() {
            return Err(format!(
                "STADO_CLOUDFLARED_BIN names no file: {}",
                path.display()
            ));
        }
        return Ok(path);
    }
    for candidate in CLOUDFLARED_CANDIDATES {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(found) = search_path("cloudflared") {
        return Ok(found);
    }
    Err(format!(
        "no cloudflared binary: set STADO_CLOUDFLARED_BIN, or install it where Stado looked \
         ({}, or anywhere on PATH). A quick tunnel needs the binary and nothing else — no \
         Cloudflare account, token or DNS record",
        CLOUDFLARED_CANDIDATES.join(", ")
    ))
}

/// Resolve the `stado` binary that will serve the enrollment routes.
///
/// The main CLI is normally the current process. The sibling and installed
/// alternatives also make this resolver usable from development harnesses that
/// execute the fleet implementation from another program.
pub fn stado_binary() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|exc| exc.to_string())?;
    if current.file_name().and_then(|name| name.to_str()) == Some("stado") {
        return Ok(current);
    }
    if let Some(sibling) = current.parent().map(|dir| dir.join("stado")) {
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let installed = Path::new(&home).join(".stado").join("bin").join("stado");
        if installed.is_file() {
            return Ok(installed);
        }
    }
    search_path("stado").ok_or_else(|| {
        "no stado binary to run the enrollment listener with: none beside this program, none at \
         $HOME/.stado/bin/stado, none on PATH"
            .to_string()
    })
}

/// First executable named `name` on `PATH`.
fn search_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}
