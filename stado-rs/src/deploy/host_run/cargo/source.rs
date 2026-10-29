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

/// The package a Cargo run of `arguments` over `manifest` is known to build:
/// the one the manifest declares (`[package] name`). None when the run
/// selects its packages itself (`-p`, `--workspace`), for a virtual
/// workspace manifest, or one that cannot be read; Cargo names those
/// members each on its own line.
pub(super) fn own_package(manifest: &Path, arguments: &[String]) -> Option<String> {
    let selects = arguments
        .iter()
        .take_while(|argument| *argument != "--")
        .any(|argument| {
            let option = argument.split('=').next().unwrap_or(argument);
            ["-p", "--package", "--workspace", "--all", "--exclude"].contains(&option)
        });
    if selects {
        return None;
    }
    let text = std::fs::read_to_string(manifest).ok()?;
    let parsed: toml::Value = toml::from_str(&text).ok()?;
    Some(parsed.get("package")?.get("name")?.as_str()?.to_string())
}

/// Whether Cargo's report says it compiled or checked `package` in this run.
pub(super) fn compiled(stderr: &str, package: &str) -> bool {
    stderr.lines().any(|line| {
        let line = line.trim_start();
        ["Compiling", "Checking"].iter().any(|verb| {
            line.strip_prefix(verb)
                .and_then(|rest| rest.trim_start().strip_prefix(package))
                .is_some_and(|rest| rest.starts_with(" v"))
        })
    })
}
