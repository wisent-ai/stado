//! Which artifact plan an installation places.
//!
//! A new installation prepares one: the exact release a pin names, or a build
//! of the product's canonical checkout. An unfinished installation (a receipt
//! left `installing`) resumes the plan it retained, so a repeated
//! after-install step places nothing new. It is superseded instead when the
//! retained plan can no longer be what the operator asked for: a pinned
//! coordinate whose files are already placed, an unpinned request over a
//! source build or a release made from another commit than the checkout now
//! stands on, or a plan prepared for a recipe the catalog
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
    /// An exact canonical commit a source build installs, instead of the
    /// checkout's head.
    pub source_commit: Option<&'a str>,
    pub id: &'a str,
}

pub(in super::super) fn decision(
    request: &Request,
    existing: Option<&ProductState>,
    dependency: bool,
) -> Result<Value> {
    let report = |action: &str, reason: String, revision: Option<String>| serde_json::json!({"action": action, "reason": reason, "source_revision": revision});
    if request.pin.is_some() {
        return Ok(report(
            "release",
            "installing the requested verified release archive".to_owned(),
            None,
        ));
    }
    if !dependency {
        let revision = match request.source_commit {
            Some(commit) if request.runtime.finishing.as_deref() != Some(request.id) => {
                let pinned = source::canonical_commit(&canonical_checkout(request)?, commit)?;
                let mut decided = report(
                    "build",
                    "explicit source installation requested".to_owned(),
                    Some(pinned.id),
                );
                decided["source_on_origin"] = serde_json::json!(pinned.on_origin);
                return Ok(decided);
            }
            commit => commit.map(str::to_owned),
        };
        return Ok(report(
            "build",
            "explicit source installation requested".to_owned(),
            revision,
        ));
    }
    let root = canonical_checkout(request)?;
    let required = match request.source_commit {
        Some(commit) => source::canonical_commit(&root, commit)?.id,
        None => source::git(&root, &["rev-parse", "HEAD"])?,
    };
    let build = |reason| report("build", reason, Some(required.clone()));
    let Some(installed) = existing else {
        return Ok(build("dependency has no installed receipt".to_owned()));
    };
    if installed.status != crate::state::InstallStatus::Installed {
        return Ok(build("dependency installation is not complete".to_owned()));
    }
    if installed.recipe != *request.selected || installed.host.as_deref() != request.host {
        return Ok(build(
            "dependency recipe or destination differs from its receipt".to_owned(),
        ));
    }
    // An installed, verified dependency is reused whatever revision it was
    // built from: a product installs against what is installed of its
    // dependencies, and what it needs of one is stated as a verb its
    // contract requires (`prerequisites`), not as the dependency's checkout
    // head. Rebuilding each dependency to its checkout head turns products
    // that depend on each other into a cycle no installation can start.
    if installed.installed_paths.is_empty() {
        return Ok(build(
            "dependency receipt names no installed files".to_owned(),
        ));
    }
    if let Err(error) = crate::install::transaction::ownership::verify(installed) {
        return Ok(build(format!(
            "dependency files failed verification: {error:#}"
        )));
    }
    Ok(report(
        "reuse",
        "dependency is installed and its files verify".to_owned(),
        installed.source_revision.clone(),
    ))
}

pub(in super::super) fn select(
    request: &Request,
    existing: Option<&ProductState>,
) -> Result<Prepared> {
    let Some(incomplete) =
        existing.filter(|state| state.status == crate::state::InstallStatus::Installing)
    else {
        if existing.is_some_and(|state| state.status.interrupted()) {
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
                request.source_commit,
            ),
        };
    };
    if incomplete.host.as_deref() != request.host {
        return Err(super::conflict::InstallationHostConflict::recorded(
            request.runtime,
            incomplete,
            request.host,
        )?
        .into());
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
                request.source_commit,
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
            // A fleet delivery (`stado release local install`) may already have
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
        // No pin asks for the canonical checkout. An unfinished installation of
        // either kind, a source build or a release, resumes only while it was
        // made from that same commit; otherwise its retained plan is not what
        // was asked for, and resuming it would repeat its failed step forever.
        None => {
            let root = canonical_checkout(request)?;
            let head = match request.source_commit {
                Some(commit) => commit.to_owned(),
                None => source::git(&root, &["rev-parse", "HEAD"])?,
            };
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
                request.source_commit,
            )?;
            supersede(request.runtime, incomplete)?;
            eprintln!(
                "{}: the unfinished installation built from {recorded} is superseded by a build \
                 of {head}; its backups stay in this installation's previous record",
                request.id
            );
            Ok(replacement)
        }
    }
}

fn supersede(runtime: &Runtime, incomplete: &ProductState) -> Result<()> {
    let mut superseded = incomplete.clone();
    superseded.status = crate::state::InstallStatus::Superseded;
    superseded.save(runtime)
}

/// The product's canonical checkout, advanced to `origin/main` when it is
/// clean and behind it. An installation that finds none creates it
/// ([`source::required_checkout`]); `sync` reads checkouts without creating
/// them.
fn canonical_checkout(request: &Request) -> Result<PathBuf> {
    let repository = request.selected["repository"]
        .as_str()
        .or(request.product["repository"].as_str())
        .context("installation has no source repository")?;
    let root = source::required_checkout(request.runtime, repository)?;
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
