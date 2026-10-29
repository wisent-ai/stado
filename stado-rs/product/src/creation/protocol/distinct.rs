//! A creation request proposes something new, so before any repository is
//! provisioned it is held against what already exists: every repository the
//! catalog claims for a product, and what GitHub answers for each repository
//! name it would create (`gh api repos/OWNER/NAME`, a read). A repository that
//! exists, or a product id that is the name of an existing repository, is
//! something we have, not a new product, and the request is refused naming it
//! and the product that claims it.
//!
//! Repositories this request itself provisioned on an earlier run (a
//! `provider/<request>/<repository>` journal entry) are its own and pass, so
//! a resumed request is not refused by its own work.

use super::super::state::Journal;
use crate::{catalog, common::capture};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};

/// The catalog product that claims each repository it names: its owner
/// repository and every surface's repository, as `catalog::claimed` reads
/// them for the whole catalog.
fn claims(document: &Value) -> Result<BTreeMap<String, String>> {
    let mut owners = BTreeMap::new();
    for product in document["products"]
        .as_array()
        .context("products must be a list")?
    {
        let id = catalog::text(product, "id")?;
        let surfaces = product["surfaces"]
            .as_array()
            .with_context(|| format!("{id}: surfaces must be a list"))?;
        let repositories = std::iter::once(catalog::text(product, "owner_repository")).chain(
            surfaces
                .iter()
                .map(|surface| catalog::text(surface, "repository")),
        );
        for repository in repositories {
            owners
                .entry(repository?.to_owned())
                .or_insert_with(|| id.to_owned());
        }
    }
    Ok(owners)
}

fn name(repository: &str) -> &str {
    repository
        .split_once('/')
        .map_or(repository, |(_, name)| name)
}

/// What GitHub answers for `repository`: `None` when it serves no
/// repository by that name to this caller (HTTP 404), else the name it
/// serves, which differs when the repository was renamed and GitHub
/// redirects the old name.
fn github(repository: &str) -> Result<Option<String>> {
    let output = capture(Command::new("gh").args(["api", &format!("repos/{repository}")]))?;
    let body: Option<Value> = serde_json::from_slice(&output.stdout).ok();
    if output.status.success() {
        return body
            .as_ref()
            .and_then(|body| body["full_name"].as_str())
            .map(|served| Some(served.to_owned()))
            .with_context(|| format!("`gh api repos/{repository}` answered without a full_name"));
    }
    if body.as_ref().is_some_and(|body| body["status"] == "404") {
        return Ok(None);
    }
    bail!(
        "`gh api repos/{repository}` failed ({}): {}; GitHub could not say whether \
         {repository} exists",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

/// One repository a request would create, and the field that named it.
pub struct Proposed<'a> {
    pub field: String,
    pub repository: &'a str,
}

/// Refuse what would not be new. `product` is the field and id of a new
/// product, absent when a surface is added to an existing one; each
/// repository is refused when the catalog claims it or GitHub serves it.
/// Returns, per repository in order, what GitHub answered: `own` when this
/// request provisioned it already, else `free`.
pub fn check(
    journal: Option<&Journal>,
    id: &str,
    product: Option<(&str, &str)>,
    repositories: &[Proposed],
    document: &Value,
) -> Result<Vec<&'static str>> {
    let mut own = BTreeSet::new();
    if let Some(journal) = journal {
        for row in repositories {
            if journal
                .get(&format!("provider/{id}/{}", row.repository))?
                .is_some()
            {
                own.insert(row.repository);
            }
        }
    }
    let claims = claims(document)?;
    if let Some((field, product)) = product {
        let owners: BTreeSet<&str> = repositories
            .iter()
            .filter_map(|row| row.repository.split_once('/').map(|(owner, _)| owner))
            .collect();
        for repository in claims.keys() {
            if name(repository) == product && !own.contains(repository.as_str()) {
                bail!(
                    "{field}: product {product} is not new: repository {repository} already \
                     exists and catalog product {} claims it; extend that product with \
                     `stado product surface add`, or register the existing repository with \
                     `stado product registry add`, instead of creating it again",
                    claims[repository]
                );
            }
        }
        for owner in owners {
            let repository = format!("{owner}/{product}");
            if own.contains(repository.as_str()) {
                continue;
            }
            if let Some(served) = github(&repository)? {
                bail!(
                    "{field}: product {product} is not new: GitHub serves {repository} as \
                     {served} and no catalog product claims it yet (`stado product catalog \
                     --unclaimed {owner}` lists it); register it with `stado product registry \
                     add` instead of creating it again"
                );
            }
        }
    }
    let mut answers = Vec::with_capacity(repositories.len());
    for row in repositories {
        let repository = row.repository;
        let field = &row.field;
        if own.contains(repository) {
            answers.push("own");
            continue;
        }
        if let Some(owner) = claims.get(repository) {
            bail!(
                "{field}: repository {repository} already belongs to catalog product {owner}; \
                 creation provisions new repositories only, so name one that does not exist yet"
            );
        }
        match github(repository)? {
            Some(served) if served.eq_ignore_ascii_case(repository) => bail!(
                "{field}: repository {repository} already exists on GitHub and no catalog \
                 product claims it (`stado product catalog --unclaimed {}` lists it); record \
                 it with `stado product registry add` for a new product or `stado product \
                 registry set PRODUCT --add-surface KIND={repository}` for an existing one \
                 instead of creating it again",
                repository
                    .split_once('/')
                    .map_or(repository, |(owner, _)| owner)
            ),
            Some(served) => bail!(
                "{field}: repository {repository} is the old name of {served}, which GitHub \
                 redirects to it; creating {repository} would break that redirect, so choose \
                 another name"
            ),
            None => answers.push("free"),
        }
    }
    Ok(answers)
}
