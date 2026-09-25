use crate::{common::Runtime, source};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    env,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct PackageSource {
    pub path: PathBuf,
    pub root: PathBuf,
    pub repository: Option<String>,
    pub revision: String,
    pub archive_sha256: Option<String>,
}

impl PackageSource {
    pub fn record(&self, name: &str) -> Value {
        json!({"package": name, "path": self.path, "source_kind": if self.archive_sha256.is_some() { "archive" } else { "git" },
            "source_directory": self.root, "repository": self.repository,
            "repository_path": self.repository.as_ref().map(|_| &self.root), "revision": self.revision, "archive_sha256": self.archive_sha256})
    }
}

fn digest(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn package(runtime: &Runtime, requested: &Path) -> Result<PackageSource> {
    let requested = crate::common::absolute(requested)?;
    let physical = requested.canonicalize()?;
    if physical != requested {
        bail!(
            "native package path is not a physical canonical path: {}",
            requested.display()
        );
    }
    let manifest = physical.join("Package.swift");
    if !manifest.is_file() {
        bail!("native package manifest is absent: {}", manifest.display());
    }
    if let Some(declared) = env::var_os("WISENT_SOURCE_DIR") {
        let root = crate::common::absolute(Path::new(&declared))?;
        if physical.starts_with(&root) && !root.join(".git").exists() {
            if root != root.canonicalize()? {
                bail!(
                    "native archive source directory is a symlink: {}",
                    root.display()
                );
            }
            let revision = env::var("WISENT_SOURCE_COMMIT")
                .context("native archive requires WISENT_SOURCE_COMMIT")?;
            let archive = env::var("WISENT_SOURCE_SHA256")
                .context("native archive requires WISENT_SOURCE_SHA256")?;
            if !digest(&revision, 40) && !digest(&revision, 64) {
                bail!("native archive requires a full WISENT_SOURCE_COMMIT");
            }
            if !digest(&archive, 64) {
                bail!("native archive requires a SHA-256 WISENT_SOURCE_SHA256");
            }
            if env::var("STADO_SOURCE_REVISION").is_ok_and(|value| value != revision) {
                bail!("STADO_SOURCE_REVISION disagrees with WISENT_SOURCE_COMMIT");
            }
            let output = env::var_os("WISENT_OUTPUT_DIR")
                .context("native archive requires WISENT_OUTPUT_DIR")?;
            if !Path::new(&output).is_absolute() {
                bail!("native archive requires an absolute WISENT_OUTPUT_DIR");
            }
            return Ok(PackageSource {
                path: physical,
                root,
                repository: None,
                revision,
                archive_sha256: Some(archive),
            });
        }
    }
    let root = PathBuf::from(source::git(&physical, &["rev-parse", "--show-toplevel"])?);
    let identity = source::repository(&source::git(&root, &["remote", "get-url", "origin"])?)
        .context("native package has no canonical GitHub origin")?;
    if source::checkout(runtime, &identity)? != root {
        bail!(
            "native package is not in its canonical Git checkout: {}",
            physical.display()
        );
    }
    let relative = manifest
        .strip_prefix(&root)?
        .to_str()
        .context("native package manifest path is not UTF-8")?;
    source::git(&root, &["ls-files", "--error-unmatch", "--", relative])?;
    let revision = source::revision(&root)?;
    Ok(PackageSource {
        path: physical,
        root,
        repository: Some(identity),
        revision,
        archive_sha256: None,
    })
}
