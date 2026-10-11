//! A pinned revision: one repository's committed tree, exported once into
//! its checkout's build area, standing in for the checkout's working tree.

use super::sources::manifest;
use crate::{common::Runtime, source};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// One repository's committed revision, exported as plain files, standing
/// in for its canonical checkout: every manifest under `exported` belongs to
/// `identity` at `commit`, whatever the checkout's working tree holds now.
#[derive(Debug, Clone)]
pub struct Pinned {
    pub identity: String,
    pub checkout: PathBuf,
    pub exported: PathBuf,
    pub commit: String,
    pub archive_sha256: String,
}

impl Pinned {
    /// The exported tree's record, in the place a checkout's snapshot
    /// takes: the commit and the digest of the archive it was unpacked from.
    pub(super) fn record(&self) -> Value {
        serde_json::json!({
            "repository": self.identity,
            "repository_path": self.checkout,
            "exported_path": self.exported,
            "commit": self.commit,
            "archive_sha256": self.archive_sha256,
            "pinned": true,
        })
    }
}

/// Export `commit` of the canonical checkout `root` into the checkout's build
/// area under Stado's home, `stado-build/<commit>/`, once: a tree already
/// there for the same commit is kept, as its marker says. The tree holds
/// only what the commit holds, so a build of it reads no session's edit. It
/// is never written into the checkout, which sits in `~/Documents` where no
/// janitor pass reaches; `stado-build` is tagged `CACHEDIR.TAG`, because
/// every export in it is reproduced from its commit.
pub fn pin(runtime: &Runtime, requested: &Path, commit: &str) -> Result<Pinned> {
    let (manifest, root, identity) = manifest(runtime, requested)?;
    let full = source::git(
        &root,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .with_context(|| {
        format!(
            "{} does not hold commit {commit}; fetch or commit it first",
            root.display()
        )
    })?;
    let exports = crate::common::runs::checkout_area(&runtime.home, &root)?.join("stado-build");
    crate::common::tag_cache(&exports, "stado product cargo --revision")?;
    let exported = exports.join(&full);
    let marker = exported.join(".stado-exported.json");
    let archive_sha256 = match fs::read(&marker) {
        Ok(bytes) => {
            let kept: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("reading {}", marker.display()))?;
            kept["archive_sha256"]
                .as_str()
                .with_context(|| format!("{} names no archive digest", marker.display()))?
                .to_owned()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if exported.exists() {
                fs::remove_dir_all(&exported)
                    .with_context(|| format!("clearing {}", exported.display()))?;
            }
            let digest = source::export(&root, &full, &exported)?;
            crate::common::atomic_json(
                &marker,
                &serde_json::json!({"commit": full, "archive_sha256": digest}),
            )?;
            digest
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", marker.display())),
    };
    let relative = manifest
        .strip_prefix(&root)
        .with_context(|| format!("{} is not under {}", manifest.display(), root.display()))?;
    if !exported.join(relative).is_file() {
        bail!(
            "commit {full} of {identity} holds no {}; the manifest is not committed there",
            relative.display()
        );
    }
    Ok(Pinned {
        identity,
        checkout: root,
        exported,
        commit: full,
        archive_sha256,
    })
}
