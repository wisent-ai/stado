//! This machine's side of one run: the checkout, commit and manifest a
//! request names, the binaries a build brought back, and the receipt of
//! everything one run did.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::deploy::DeployError;

use super::{refused, CargoOperation};

/// One binary a build brought back: where it now is, its size and digest.
#[derive(Debug, Serialize)]
pub struct Artifact {
    pub binary: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

/// Everything one run did, as `--json` prints it.
#[derive(Debug, Serialize)]
pub struct HostCargoReceipt {
    pub schema: &'static str,
    pub target: String,
    pub checkout: PathBuf,
    pub commit: String,
    /// SHA-256 of the archive the commit was exported from, the
    /// `WISENT_SOURCE_SHA256` the build was given beside the commit.
    pub archive_sha256: String,
    pub manifest: String,
    pub operation: CargoOperation,
    pub binaries: Vec<String>,
    pub arguments: Vec<String>,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub artifacts: Vec<Artifact>,
    /// Free bytes on the volume holding the host's build cache before and
    /// after, from `df -P` there (`volume`).
    pub free_bytes_before: Option<u64>,
    pub free_bytes_after: Option<u64>,
    pub free_bytes_after_error: Option<String>,
    /// The run directory the revision was delivered into; removed before
    /// the receipt is written.
    pub run_directory: String,
}

impl HostCargoReceipt {
    /// The volume's free space this run took, in GiB: what the two readings
    /// differ by, other writers on the volume included.
    pub fn consumed_gib(&self) -> Option<f64> {
        let (Some(before), Some(after)) = (self.free_bytes_before, self.free_bytes_after) else {
            return None;
        };
        let gib = crate::providers::local::host_memory::constants::GIB as f64;
        Some((before as f64 - after as f64) / gib)
    }
}

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
/// and the manifest's path relative to the checkout. The manifest is read
/// from the commit, never from the working tree: a revision that adds or
/// moves its `Cargo.toml` names a file the working tree may not hold, so
/// only the nearest directory of the path that exists is resolved on disk
/// (it finds the checkout), and the commit must hold the manifest itself.
pub(super) async fn source(
    manifest: &Path,
    revision: &str,
) -> Result<(PathBuf, String, String), DeployError> {
    let (existing, rest) = existing_ancestor(manifest)?;
    let checkout = PathBuf::from(git(&existing, &["rev-parse", "--show-toplevel"]).await?);
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
    let relative = existing
        .join(rest)
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
    git(
        &checkout,
        &["cat-file", "-e", &format!("{commit}:{relative}")],
    )
    .await
    .map_err(|_| refused(format!("{commit} holds no {relative} to run Cargo with")))?;
    Ok((checkout, commit, relative))
}

/// The nearest ancestor of `path` that exists, resolved, and the part of
/// `path` below it.
fn existing_ancestor(path: &Path) -> Result<(PathBuf, PathBuf), DeployError> {
    let mut rest = PathBuf::new();
    let mut at = path;
    loop {
        match std::fs::canonicalize(at) {
            Ok(resolved) if resolved.is_dir() => return Ok((resolved, rest)),
            Ok(resolved) => {
                let parent = resolved.parent().unwrap_or(Path::new("/")).to_path_buf();
                let name = resolved.file_name().map_or(PathBuf::new(), PathBuf::from);
                return Ok((parent, name.join(rest)));
            }
            Err(_) => {
                let Some(name) = at.file_name() else {
                    return Err(refused(format!(
                        "no directory of {} exists on this machine to find its checkout from",
                        path.display()
                    )));
                };
                rest = PathBuf::from(name).join(rest);
                at = at.parent().unwrap_or(Path::new(""));
            }
        }
    }
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
