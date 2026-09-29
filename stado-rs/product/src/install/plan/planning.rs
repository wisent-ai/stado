//! Which artifact plan an installation places.
//!
//! A new installation prepares one: the exact release a pin names, or a build
//! of the product's canonical checkout. An unfinished installation (a receipt
//! left `installing`) resumes the plan it retained, so a repeated
//! after-install step places nothing new. It is superseded instead when the
//! retained plan can no longer be what the operator asked for: a pinned
//! coordinate whose files are already placed, a source build whose checkout
//! now stands on another commit, or a plan prepared for a recipe the catalog
//! has since corrected. Resuming such a plan would repeat whatever it got
//! wrong on every attempt, with a rollback as the only way on.

use super::{release, Prepared};
use crate::install::recipes;
use crate::{common::Runtime, source, state::ProductState};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

pub(in super::super) struct Request<'a> {
    pub runtime: &'a Runtime,
    pub product: &'a Value,
    pub selected: &'a Value,
    pub surface: &'a str,
    pub host: Option<&'a str>,
    pub pin: Option<(&'a str, &'a str)>,
    pub id: &'a str,
}

pub(in super::super) fn select(
    request: &Request,
    existing: Option<&ProductState>,
) -> Result<Prepared> {
    let Some(incomplete) = existing.filter(|state| state.status == "installing") else {
        if existing
            .is_some_and(|state| state.status == "removing" || state.status == "rolling_back")
        {
            bail!("finish the recorded removal or rollback before installing");
        }
        return match request.pin {
            Some((version, revision)) => {
                release::prepare(request.product, version, revision, request.runtime)
            }
            None => recipes::prepare(
                request.runtime,
                request.product,
                request.selected,
                request.surface,
                &canonical_checkout(request)?,
            ),
        };
    };
    if incomplete.host.as_deref() != request.host {
        bail!("unfinished installation is bound to another host; finish it there first");
    }
    // A catalog correction changes the recipe itself (for example a
    // host_config key the host now refuses). The retained plan was prepared
    // for the old recipe, so resuming it repeats the refused step; it is
    // superseded by a fresh preparation of the recipe asked for now.
    if incomplete.recipe != *request.selected {
        let replacement = match request.pin {
            Some((version, revision)) => {
                release::prepare(request.product, version, revision, request.runtime)?
            }
            None => recipes::prepare(
                request.runtime,
                request.product,
                request.selected,
                request.surface,
                &canonical_checkout(request)?,
            )?,
        };
        supersede(request.runtime, incomplete)?;
        eprintln!(
            "{}: the unfinished installation prepared for an earlier recipe is superseded by \
             the current catalog recipe; its backups stay in this installation's previous record",
            request.id
        );
        return Ok(replacement);
    }
    let retained = || -> Result<Prepared> {
        Ok(serde_json::from_value(
            incomplete
                .extra
                .get("prepared")
                .cloned()
                .context("interrupted installation has no retained artifact plan")?,
        )?)
    };
    match request.pin {
        Some((version, revision)) => {
            let accepted = incomplete
                .release
                .as_ref()
                .context("unfinished installation was not an exact release")?;
            if accepted["coordinate"]["version"] == version
                && accepted["coordinate"]["source_revision"] == revision
            {
                return retained();
            }
            // A fleet delivery (`stado release install-local`) may already have
            // placed the requested release over an install that stopped in its
            // after-install step; then the unfinished record is superseded, not
            // lost: it becomes this installation's `previous`.
            let replacement =
                release::prepare(request.product, version, revision, request.runtime)?;
            if !already_placed(&replacement)? {
                bail!("unfinished installation is bound to another release coordinate; roll it back first");
            }
            supersede(request.runtime, incomplete)?;
            eprintln!(
                "{}: the unfinished installation of {} is superseded: every file it placed \
                 already holds {version} ({revision})",
                request.id, accepted["coordinate"]["version"]
            );
            Ok(replacement)
        }
        None if incomplete.release.is_none() => {
            let root = canonical_checkout(request)?;
            let head = source::git(&root, &["rev-parse", "HEAD"])?;
            let recorded = incomplete.source_revision.as_deref().unwrap_or_default();
            if recorded == head {
                return retained();
            }
            let replacement = recipes::prepare(
                request.runtime,
                request.product,
                request.selected,
                request.surface,
                &root,
            )?;
            supersede(request.runtime, incomplete)?;
            eprintln!(
                "{}: the unfinished installation built from {recorded} is superseded by a build \
                 of {head}; its backups stay in this installation's previous record",
                request.id
            );
            Ok(replacement)
        }
        None => retained(),
    }
}

fn supersede(runtime: &Runtime, incomplete: &ProductState) -> Result<()> {
    let mut superseded = incomplete.clone();
    superseded.status = "superseded".to_owned();
    superseded.save(runtime)
}

/// The product's canonical checkout, advanced to `origin/main` when it is
/// clean and behind it.
fn canonical_checkout(request: &Request) -> Result<PathBuf> {
    let repository = request.selected["repository"]
        .as_str()
        .or(request.product["repository"].as_str())
        .context("installation has no source repository")?;
    let root = source::checkout(request.runtime, repository)?;
    if source::git(&root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        let ancestor = crate::common::capture(
            Command::new("git")
                .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
                .current_dir(&root),
        )?;
        if ancestor.status.success() {
            source::advance(&root, false)?;
        }
    }
    Ok(root)
}

/// Whether every placement of `plan` already holds exactly what it would
/// place: the same bytes for a file, the same target for a link.
fn already_placed(plan: &Prepared) -> Result<bool> {
    for placement in &plan.placements {
        let holds = if placement.symbolic {
            std::fs::read_link(&placement.destination).ok().as_deref()
                == Some(placement.source.as_path())
        } else {
            placement.destination.is_file()
                && crate::common::sha256(&placement.destination)?
                    == crate::common::sha256(&placement.source)?
        };
        if !holds {
            return Ok(false);
        }
    }
    Ok(!plan.placements.is_empty())
}
