//! Cross-product verb declarations live in a committed contract beside the
//! strict release manifest's directory. A predecessor installer ignores it;
//! this installer reads it at each exact provider or consumer revision.

use super::Requirement;
use crate::source;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

const CONTRACT: &str = "contracts/verbs.json";

#[derive(Deserialize)]
struct ReleaseIdentity {
    product: String,
}

#[derive(Deserialize)]
pub(super) struct Manifest {
    pub(super) product: String,
    #[serde(default)]
    provides: Vec<Provided>,
    #[serde(default)]
    pub(super) requires: Vec<Requirement>,
}

#[derive(Deserialize)]
struct Provided {
    verb: String,
}

fn path_beside(release: &str) -> String {
    let path = match Path::new(release).parent() {
        Some(parent) => parent.join(CONTRACT),
        None => Path::new(CONTRACT).to_path_buf(),
    };
    path.display().to_string()
}

pub(super) fn manifest(root: &Path, revision: &str, release_path: &str) -> Result<Manifest> {
    let reference = format!("{revision}:{release_path}");
    let content = source::git(root, &["show", &reference])
        .with_context(|| format!("reading release manifest {release_path} at {revision}"))?;
    let release: ReleaseIdentity = serde_json::from_str(&content)
        .with_context(|| format!("parsing release manifest {release_path} at {revision}"))?;
    let contract_path = path_beside(release_path);
    let listed = source::git(
        root,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            revision,
            "--",
            &contract_path,
        ],
    )?;
    if !listed.lines().any(|line| line == contract_path.as_str()) {
        return Ok(Manifest {
            product: release.product,
            provides: Vec::new(),
            requires: Vec::new(),
        });
    }
    let reference = format!("{revision}:{contract_path}");
    let content = source::git(root, &["show", &reference])
        .with_context(|| format!("reading verb contract {contract_path} at {revision}"))?;
    let declared: Manifest = serde_json::from_str(&content)
        .with_context(|| format!("parsing verb contract {contract_path} at {revision}"))?;
    if declared.product != release.product {
        bail!(
            "verb contract {contract_path} at {revision} declares {} instead of {}",
            declared.product,
            release.product
        );
    }
    Ok(declared)
}

pub(super) fn provides(manifest: &Manifest, verb: &str) -> bool {
    manifest
        .provides
        .iter()
        .any(|provided| provided.verb == verb)
}

pub(super) fn first_provider(
    root: &Path,
    revision: &str,
    release_path: &str,
    verb: &str,
) -> Result<String> {
    let contract_path = path_beside(release_path);
    let pickaxe = format!("-S{verb}");
    let history = source::git(
        root,
        &[
            "log",
            "--reverse",
            "--format=%H",
            &pickaxe,
            revision,
            "--",
            &contract_path,
        ],
    )?;
    for candidate in history.lines() {
        let declared = manifest(root, candidate, release_path)?;
        if provides(&declared, verb) {
            return Ok(candidate.to_owned());
        }
    }
    bail!("provider has no source revision declaring the verb {verb} at {revision}")
}
