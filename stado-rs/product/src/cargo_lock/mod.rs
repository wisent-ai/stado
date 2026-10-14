//! Whether a committed Cargo workspace's lock covers what its manifests
//! declare.
//!
//! A build runs `cargo build --locked` in the exported tree of one commit,
//! and Cargo refuses a `Cargo.lock` that lacks a dependency the manifest
//! declares. Its refusal names neither the commit nor the dependency the
//! lock lacks, and Stado passed it on unclassified, so a manifest committed
//! without its lock read as a request or credential problem. A failed build
//! step is therefore read against the tree it built: each workspace's
//! declared dependencies (`cargo metadata --no-deps`, which resolves
//! nothing) are compared with the packages its committed `Cargo.lock`
//! names, and a dependency the lock lacks is named with the revision and
//! the manifest, as a fault of that committed source.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// The Cargo workspaces a build of `tree` could compile: a `Cargo.toml` with
/// a `Cargo.lock` beside it, at the root or one directory down
/// (`<product>-rs` layouts), in path order. A tree that cannot be listed is
/// an error: read as holding no workspace, every lock in it would pass unasked.
pub fn workspaces(tree: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut consider = |dir: &Path| {
        if dir.join("Cargo.toml").is_file() && dir.join("Cargo.lock").is_file() {
            found.push(dir.to_path_buf());
        }
    };
    consider(tree);
    let mut children = Vec::new();
    for entry in std::fs::read_dir(tree)? {
        let path = entry?.path();
        if path.is_dir() {
            children.push(path);
        }
    }
    children.sort();
    for child in children {
        consider(&child);
    }
    Ok(found)
}

/// A build that failed on a committed manifest its own `Cargo.lock` does not
/// cover: the product, the revision, the manifest within the commit and
/// every dependency the lock lacks, with the build's own failure.
#[derive(Debug)]
pub struct UnlockedManifest {
    pub product: String,
    pub revision: String,
    pub manifest: PathBuf,
    pub dependencies: Vec<String>,
    pub cause: String,
}

impl fmt::Display for UnlockedManifest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lock = self.manifest.with_file_name("Cargo.lock");
        write!(
            formatter,
            "committed source configuration: {} at {}: {} declares {} that its committed {} \
             does not lock; commit {} updated for {} \
             (cargo update -w --manifest-path {}) in {} and install again. The build said: {}",
            self.product,
            self.revision,
            self.manifest.display(),
            self.dependencies.join(", "),
            lock.display(),
            lock.display(),
            self.manifest.display(),
            self.manifest.display(),
            self.product,
            self.cause
        )
    }
}

impl std::error::Error for UnlockedManifest {}

/// The dependencies the manifests of `workspace` declare that its
/// `Cargo.lock` names no package for, including newly added path packages.
pub fn unlocked(workspace: &Path) -> Result<Vec<String>> {
    let manifest = workspace.join("Cargo.toml");
    let output = stado_wait::output(
        Command::new(crate::common::step_program("cargo"))
            .args([
                "metadata",
                "--no-deps",
                "--offline",
                "--format-version",
                "1",
                "--manifest-path",
            ])
            .arg(&manifest)
            .current_dir(workspace)
            .stdin(Stdio::null()),
    )
    .with_context(|| {
        format!(
            "cannot run cargo metadata --no-deps for {}",
            manifest.display()
        )
    })?;
    if !output.status.success() {
        bail!(
            "cargo metadata --no-deps for {} exited {}: {}",
            manifest.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("cargo metadata for {} printed no JSON", manifest.display()))?;
    let lock_path = workspace.join("Cargo.lock");
    let lock: toml::Value = toml::from_str(
        &std::fs::read_to_string(&lock_path)
            .with_context(|| format!("cannot read {}", lock_path.display()))?,
    )
    .with_context(|| format!("{} is not TOML", lock_path.display()))?;
    let locked: BTreeSet<&str> = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .with_context(|| format!("{} names no [[package]]", lock_path.display()))?
        .iter()
        .filter_map(|package| package.get("name").and_then(toml::Value::as_str))
        .collect();
    let packages = metadata["packages"].as_array().with_context(|| {
        format!(
            "cargo metadata for {} listed no packages",
            manifest.display()
        )
    })?;
    let mut missing = BTreeSet::new();
    for package in packages {
        let declaring = package["manifest_path"].as_str().with_context(|| {
            format!(
                "cargo metadata for {} omitted a package manifest",
                manifest.display()
            )
        })?;
        let declaring = Path::new(declaring)
            .strip_prefix(workspace)
            .with_context(|| {
                format!(
                    "declaring manifest {declaring} is outside {}",
                    workspace.display()
                )
            })?;
        for dependency in package["dependencies"].as_array().into_iter().flatten() {
            let Some(name) = dependency["name"].as_str() else {
                bail!(
                    "cargo metadata for {} lists a dependency of {} without a name",
                    manifest.display(),
                    package["name"]
                );
            };
            if !locked.contains(name) {
                missing.insert(format!("{name} (declared in {})", declaring.display()));
            }
        }
    }
    Ok(missing.into_iter().collect())
}

/// Read a failed build of `tree` against its committed locks: the first
/// workspace whose lock lacks a declared dependency turns `failure` into an
/// [`UnlockedManifest`]; otherwise `failure` stands, with why a lock could
/// not be read when one could not.
pub fn explain(
    failure: anyhow::Error,
    tree: &Path,
    product: &str,
    revision: &str,
) -> anyhow::Error {
    let workspaces = match workspaces(tree) {
        Ok(workspaces) => workspaces,
        Err(error) => {
            return failure.context(format!(
                "whether the committed Cargo.lock files of {product} cover their manifests could \
                 not be read: cannot list {}: {error}",
                tree.display()
            ))
        }
    };
    for workspace in workspaces {
        let manifest = workspace.join("Cargo.toml");
        let relative = match manifest.strip_prefix(tree) {
            Ok(relative) => relative.to_path_buf(),
            Err(error) => {
                return failure.context(format!(
                    "the workspace manifest {} is not inside the built tree {}: {error}",
                    manifest.display(),
                    tree.display()
                ))
            }
        };
        match unlocked(&workspace) {
            Ok(dependencies) if dependencies.is_empty() => {}
            Ok(dependencies) => {
                return anyhow::Error::new(UnlockedManifest {
                    product: product.to_string(),
                    revision: revision.to_string(),
                    manifest: relative,
                    dependencies,
                    cause: format!("{failure:#}"),
                })
            }
            Err(error) => {
                return failure.context(format!(
                    "whether {}'s committed Cargo.lock covers it could not be read: {error:#}",
                    relative.display()
                ))
            }
        }
    }
    failure
}
