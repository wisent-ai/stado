//! The version step: a commit carrying repairs under a version a release of
//! another commit already holds gets the least next version the versioning
//! rule allows (`least_next`, AutoVersion's step for an internal change),
//! committed in the product's own source where its manifest's
//! `version_source` declares it — the number is the rule's, never invented
//! here. A version source in a `Cargo.toml` moves the workspace's own entry
//! in its `Cargo.lock` with it, through cargo, offline, so the committed
//! revision still builds with `--locked`.
//!
//! The commit holds the version file and that lock and nothing else. A
//! checkout whose version file or lock carries someone's uncommitted change,
//! or whose HEAD moved past the commit the loop planned, is refused by name:
//! the loop never commits work that is not its own.

use std::path::{Path, PathBuf};

use super::process::run;
use crate::cli::CmdError;
use crate::release_pipeline::{
    parse_product_manifest, version_path, with_version, ProductManifest, PRODUCT_MANIFEST,
};

/// The manifest of a Rust package, whose version its lock repeats.
const CARGO_MANIFEST: &str = "Cargo.toml";
const CARGO_LOCK: &str = "Cargo.lock";

/// The lock nearest above `manifest` inside `checkout`, as a path relative
/// to the checkout, when there is one.
fn lock_of(checkout: &Path, manifest: &Path) -> Result<Option<String>, CmdError> {
    let found = manifest
        .ancestors()
        .skip_while(|directory| directory == &manifest)
        .take_while(|directory| directory.starts_with(checkout))
        .map(|directory| directory.join(CARGO_LOCK))
        .find(|lock| lock.is_file());
    let Some(lock) = found else {
        return Ok(None);
    };
    let relative: PathBuf = lock
        .strip_prefix(checkout)
        .map_err(|_| {
            CmdError::click(format!(
                "{} is not inside {}",
                lock.display(),
                checkout.display()
            ))
        })?
        .to_path_buf();
    Ok(Some(relative.display().to_string()))
}

/// The release manifest of `checkout`'s product.
fn manifest_of(
    checkout: &Path,
    product: &str,
) -> Result<crate::release_pipeline::ReleasePipelineManifest, CmdError> {
    let path = checkout.join(PRODUCT_MANIFEST);
    let bytes = std::fs::read(&path)
        .map_err(|error| CmdError::click(format!("reading {}: {error}", path.display())))?;
    match parse_product_manifest(&bytes)
        .map_err(|error| CmdError::click(format!("{}: {error}", path.display())))?
    {
        ProductManifest::Release(manifest) => Ok(manifest),
        ProductManifest::NonRelease(_) => Err(CmdError::click(format!(
            "{} declares that {product} does not release",
            path.display()
        ))),
    }
}

/// Whether `checkout`'s manifest declares a format gate on any platform:
/// a product without one has nothing for the queue's compile-free gate step
/// to run, and its release pipeline runs whatever it declares.
pub fn declares_format_gate(checkout: &Path, product: &str) -> Result<bool, CmdError> {
    Ok(manifest_of(checkout, product)?
        .platforms
        .values()
        .flat_map(|platform| &platform.quality)
        .any(|gate| gate.kind == crate::release_pipeline::GateKind::Format))
}

/// Commit `next` as `product`'s version in `checkout`, standing at `commit`,
/// and answer the new commit.
pub async fn commit_next(
    checkout: &Path,
    product: &str,
    commit: &str,
    next: &str,
) -> Result<String, CmdError> {
    let head = run("git", &["rev-parse", "HEAD"], Some(checkout)).await?;
    if head.trim() != commit {
        return Err(CmdError::click(format!(
            "{} moved from {commit} to {} while the loop planned it; the next pass takes the new commit",
            checkout.display(),
            head.trim()
        )));
    }
    let manifest = manifest_of(checkout, product)?;
    let relative = version_path(&manifest.version_source).to_string();
    let file = checkout.join(&relative);
    let mut paths = vec![relative.clone()];
    let in_cargo = file.file_name().and_then(|name| name.to_str()) == Some(CARGO_MANIFEST);
    let lock = match in_cargo {
        true => lock_of(checkout, &file)?,
        false => None,
    };
    paths.extend(lock.clone());
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend(paths.iter().map(String::as_str));
    let dirty = run("git", &status, Some(checkout)).await?;
    if !dirty.trim().is_empty() {
        return Err(CmdError::click(format!(
            "{} holds uncommitted changes to the files the version commit would carry, which are not the loop's to commit: {}",
            checkout.display(),
            dirty.trim().replace('\n', "; ")
        )));
    }
    let bytes = std::fs::read(&file)
        .map_err(|error| CmdError::click(format!("reading {}: {error}", file.display())))?;
    let written = with_version(&manifest.version_source, &bytes, next)
        .map_err(|error| CmdError::click(format!("{}: {error}", file.display())))?;
    std::fs::write(&file, written)
        .map_err(|error| CmdError::click(format!("writing {}: {error}", file.display())))?;
    if lock.is_some() {
        let cargo_manifest = file.display().to_string();
        let locked = run(
            "cargo",
            &[
                "update",
                "--workspace",
                "--offline",
                "--manifest-path",
                &cargo_manifest,
            ],
            Some(checkout),
        )
        .await;
        if let Err(error) = locked {
            let mut restore = vec!["checkout", "--"];
            restore.extend(paths.iter().map(String::as_str));
            run("git", &restore, Some(checkout)).await?;
            return Err(error);
        }
    }
    let message = format!(
        "Release {product} {next}\n\nThe least version the versioning rule allows after a released one, so the repairs this source carries can be released."
    );
    let mut commit_args = vec!["commit", "-q", "-m", &message, "--"];
    commit_args.extend(paths.iter().map(String::as_str));
    run("git", &commit_args, Some(checkout)).await?;
    Ok(run("git", &["rev-parse", "HEAD"], Some(checkout))
        .await?
        .trim()
        .to_string())
}
