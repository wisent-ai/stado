//! The catalog an installation executes, read from a committed revision.
//!
//! An installation runs fields of its recipe — the build, `after_install`,
//! the service it ensures — so the recipe it reads is code it executes. The
//! working tree of the catalog's checkout is whatever an executor has half
//! written: an install that read it ran an `after_install` verb renamed in an
//! uncommitted edit, against a binary built from a revision that still had the
//! old one. So an installation reads `catalog/products.yml` from a commit: the
//! installed revision when the installed product is the repository the catalog
//! lives in (Stado itself), else the checkout's committed `HEAD`. The record
//! of what was read — path, revision and why that revision — is kept on the
//! installation and printed in its report.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

use crate::common::Runtime;
use crate::source::git;

/// Where the catalog sits in Stado's own source tree; the file
/// [`super::EMBEDDED`] compiles in.
const IN_SOURCE: &str = "catalog/products.yml";
/// Where the catalog an installation executes came from.
#[derive(Clone, Debug, Serialize)]
pub struct Provenance {
    /// The catalog's checkout and path within it, or `embedded`.
    pub source: String,
    /// The commit it was read at.
    pub revision: String,
    /// Why that commit.
    pub chosen: Choice,
}

/// Why the catalog was read at its revision.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Choice {
    /// The installed product is the catalog's own repository, read at the
    /// revision being installed.
    InstalledRevision,
    /// The committed head of the catalog's checkout.
    CommittedHead,
    /// No catalog checkout: the catalog compiled into this binary.
    Embedded,
}

/// The catalog for an installation of `product`, at `revision` when the
/// product is the catalog's own repository and a revision is named.
pub fn for_installation(
    runtime: &Runtime,
    product: &str,
    revision: Option<&str>,
) -> Result<(Value, Provenance)> {
    if runtime.embedded_catalog {
        return Ok((
            super::embedded()?,
            Provenance {
                source: "embedded".to_owned(),
                revision: crate::build().source_revision.to_owned(),
                chosen: Choice::Embedded,
            },
        ));
    }
    // The installed revision's own catalog sits at its in-source path in the
    // Stado checkout, whatever `--catalog` names; the committed head is read
    // at the path of the catalog this command was given.
    let (file, relative, wanted, chosen) =
        match revision.filter(|_| product == crate::build().product) {
            Some(revision) => (
                runtime.workspace.join(crate::common::CATALOG),
                Some(IN_SOURCE),
                revision,
                Choice::InstalledRevision,
            ),
            None => (runtime.catalog.clone(), None, "HEAD", Choice::CommittedHead),
        };
    let directory = file
        .parent()
        .with_context(|| format!("the catalog {} has no directory", file.display()))?;
    let root = git(directory, &["rev-parse", "--show-toplevel"]).with_context(|| {
        format!(
            "the catalog {} is not in a git checkout, so no committed revision of it can be read",
            file.display()
        )
    })?;
    let root = Path::new(&root);
    let relative = match relative {
        Some(relative) => relative.to_owned(),
        None => file
            .canonicalize()
            .with_context(|| format!("reading {}", file.display()))?
            .strip_prefix(root.canonicalize()?)
            .with_context(|| {
                format!(
                    "the catalog {} lies outside its checkout {}",
                    file.display(),
                    root.display()
                )
            })?
            .to_string_lossy()
            .into_owned(),
    };
    let commit = git(
        root,
        &["rev-parse", "--verify", &format!("{wanted}^{{commit}}")],
    )
    .with_context(|| format!("{wanted} names no commit in {}", root.display()))?;
    let text = git(root, &["show", &format!("{commit}:{relative}")])
        .with_context(|| format!("{} holds no {relative} at {commit}", root.display()))?;
    let document: Value = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing {relative} at {commit} of {}", root.display()))?;
    super::validate(&document)
        .with_context(|| format!("{relative} at {commit} of {}", root.display()))?;
    Ok((
        document,
        Provenance {
            source: format!("{}:{relative}", root.display()),
            revision: commit,
            chosen,
        },
    ))
}
