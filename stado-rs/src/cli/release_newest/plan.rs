//! What the workspace holds, and what of it is worth releasing right now.
//!
//! Reading comes before spending. Every product checkout under the workspace
//! is read for three facts — the commit it stands on, the version that commit
//! declares, and whether that version was already published — and only what
//! passes all three is submitted. A product that declares `releases: false`,
//! a checkout whose manifest names a version the store already carries, and a
//! checkout that cannot be read at all are each reported with the reason, in
//! the same listing, instead of disappearing from it.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::release_submit::{
    committed_file, published_coordinates, resolve_commit, VERSION_SCAN_WINDOW,
};
use crate::cli::CmdError;
use crate::release_pipeline::{self, ProductManifest, PRODUCT_MANIFEST};

/// One name for what a published run is keyed by.
type Published = std::collections::BTreeMap<(String, String), String>;

/// What one product checkout is, as far as releasing is concerned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "standing", rename_all = "snake_case")]
pub enum Standing {
    /// Its declared version has no run yet: this is what `newest` submits.
    Releasable { commit: String, version: String },
    /// Its declared version already has a run, so there is nothing to cut.
    Published {
        commit: String,
        version: String,
        run: String,
    },
    /// The product declares that it does not release, and says why.
    DeclaresNoReleases { reason: String },
    /// The checkout could not be read far enough to decide.
    Unreadable { refusal: String },
}

/// One product checkout of the workspace and what is to be done with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Planned {
    pub product: String,
    /// The checkout, relative to the workspace root when it lies under it.
    pub checkout: PathBuf,
    #[serde(flatten)]
    pub standing: Standing,
}

impl Planned {
    pub fn is_releasable(&self) -> bool {
        matches!(self.standing, Standing::Releasable { .. })
    }
}

/// Every product checkout under `root`, in name order, with its standing.
///
/// `products` selects by product name; empty means the whole workspace. A
/// name that matches nothing is a refusal, not an empty run: a release that
/// silently did nothing reads exactly like a release that worked.
pub async fn plan(root: &Path, products: &[String]) -> Result<Vec<Planned>, CmdError> {
    let mut checkouts = Vec::new();
    scan(root, &mut checkouts)?;
    checkouts.sort();
    if checkouts.is_empty() {
        return Err(CmdError::click(format!(
            "{} holds no product checkout: nothing under it carries a {PRODUCT_MANIFEST}",
            root.display()
        )));
    }
    let published = published_coordinates(VERSION_SCAN_WINDOW).await?;
    let mut planned = Vec::new();
    for checkout in checkouts {
        let entry = read(&checkout, &published);
        if !products.is_empty() && !products.contains(&entry.product) {
            continue;
        }
        planned.push(entry);
    }
    for wanted in products {
        if !planned.iter().any(|entry| &entry.product == wanted) {
            return Err(CmdError::click(format!(
                "{} holds no checkout of product {wanted:?}",
                root.display()
            )));
        }
    }
    planned.sort_by(|left, right| left.product.cmp(&right.product));
    Ok(planned)
}

/// Read one checkout: its product, its commit, its declared version, and
/// whether that version has a run already.
fn read(checkout: &Path, published: &Published) -> Planned {
    let product = match product_name(checkout) {
        Ok(name) => name,
        Err(refusal) => {
            return Planned {
                product: checkout
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| checkout.display().to_string()),
                checkout: checkout.to_path_buf(),
                standing: Standing::Unreadable { refusal },
            }
        }
    };
    let standing = match standing(checkout, published) {
        Ok(standing) => standing,
        Err(refusal) => Standing::Unreadable { refusal },
    };
    Planned {
        product,
        checkout: checkout.to_path_buf(),
        standing,
    }
}

fn product_name(checkout: &Path) -> Result<String, String> {
    let bytes =
        std::fs::read(checkout.join(PRODUCT_MANIFEST)).map_err(|error| error.to_string())?;
    let manifest = release_pipeline::parse_product_manifest(&bytes)?;
    Ok(match manifest {
        ProductManifest::Release(value) => value.product,
        ProductManifest::NonRelease(value) => value.product,
    })
}

fn standing(checkout: &Path, published: &Published) -> Result<Standing, String> {
    let commit = resolve_commit(checkout, None).map_err(|error| error.to_string())?;
    let bytes =
        committed_file(checkout, &commit, PRODUCT_MANIFEST).map_err(|error| error.to_string())?;
    let manifest = release_pipeline::parse_product_manifest(&bytes)?;
    let manifest = match manifest {
        ProductManifest::Release(value) => value,
        ProductManifest::NonRelease(value) => {
            return Ok(Standing::DeclaresNoReleases {
                reason: value.reason,
            })
        }
    };
    let version = release_pipeline::declared_version(&manifest.version_source, |path| {
        committed_file(checkout, &commit, path).map_err(|error| error.to_string())
    })?;
    match published.get(&(manifest.product.clone(), version.clone())) {
        Some(run) => Ok(Standing::Published {
            commit,
            version,
            run: run.clone(),
        }),
        None => Ok(Standing::Releasable { commit, version }),
    }
}

/// Every checkout under `root` that declares a product.
///
/// The walk stops at a checkout: a release declaration belongs to the tree
/// that holds it, and a dependency clone or a build directory inside that
/// tree is not another product the operator checked out.
fn scan(root: &Path, found: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    if root.join(PRODUCT_MANIFEST).is_file() {
        found.push(root.to_path_buf());
        return Ok(());
    }
    if root.join(".git").exists() {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(root)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".build" | ".git" | "target" | "node_modules" | ".venv" | "dist" | "build"
        ) {
            continue;
        }
        scan(&entry.path(), found)?;
    }
    Ok(())
}
