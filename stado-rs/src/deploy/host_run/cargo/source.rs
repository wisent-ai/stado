//! This machine's side of one run: the checkout, commit and manifest a
//! request names, and the binaries a build brought back.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::deploy::DeployError;

use super::{refused, Artifact};

async fn git(directory: &Path, arguments: &[&str]) -> Result<String, DeployError> {
    let output = crate::wait::output_async(
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments),
    )
    .await
    .map_err(DeployError::io(format!(
        "cannot run git in {}",
        directory.display()
    )))?;
    if !output.status.success() {
        return Err(refused(format!(
            "git {} in {} failed: {}",
            arguments.join(" "),
            directory.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The checkout holding `manifest`, the full commit `revision` names in it,
/// and the manifest's path relative to the checkout.
pub(super) async fn source(
    manifest: &Path,
    revision: &str,
) -> Result<(PathBuf, String, String), DeployError> {
    let manifest = std::fs::canonicalize(manifest).map_err(DeployError::io(format!(
        "the manifest {} cannot be read",
        manifest.display()
    )))?;
    let directory = manifest.parent().unwrap_or(Path::new("/"));
    let checkout = PathBuf::from(git(directory, &["rev-parse", "--show-toplevel"]).await?);
    let commit = git(
        &checkout,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )
    .await
    .map_err(|_| {
        refused(format!(
            "{} does not hold commit {revision}; fetch or commit it first",
            checkout.display()
        ))
    })?;
    let relative = manifest
        .strip_prefix(&checkout)
        .map_err(|_| {
            refused(format!(
                "{} is not inside {}",
                manifest.display(),
                checkout.display()
            ))
        })?
        .to_string_lossy()
        .into_owned();
    Ok((checkout, commit, relative))
}

/// One binary a build pulled into `directory`, with its size and digest.
pub(super) fn artifact(directory: &Path, binary: &str) -> Result<Artifact, DeployError> {
    let path = directory.join(binary);
    let bytes = std::fs::read(&path).map_err(DeployError::io(format!(
        "the built {binary} was not brought back to {}",
        path.display()
    )))?;
    Ok(Artifact {
        binary: binary.to_string(),
        bytes: bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(&bytes)),
        path,
    })
}
