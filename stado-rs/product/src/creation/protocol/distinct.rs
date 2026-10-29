//! A creation request proposes something new, so before any repository is
//! provisioned it is held against what already exists: every repository the
//! catalog claims for a product, and every repository each requested owner
//! already holds on GitHub (the list `stado product catalog --unclaimed`
//! reads). A repository that exists, or a product id that is the name of an
//! existing repository, is something we have, not a new product, and the
//! request is refused naming it and the product that claims it.
//!
//! Repositories this request itself provisioned on an earlier run (a
//! `provider/<request>/<repository>` journal entry) are its own and pass, so
//! a resumed request is not refused by its own work.

use super::super::state::Journal;
use super::Request;
use crate::catalog;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

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

/// Refuse `request` when what it would create already exists.
pub fn check(journal: &Journal, request: &Request, document: &Value) -> Result<()> {
    let id = &request.request_id;
    let mut own = BTreeSet::new();
    for row in &request.repositories {
        if journal
            .get(&format!("provider/{id}/{}", row.repository))?
            .is_some()
        {
            own.insert(row.repository.as_str());
        }
    }
    let claims = claims(document)?;
    let mut existing = BTreeSet::new();
    let owners: BTreeSet<&str> = request
        .repositories
        .iter()
        .filter_map(|row| row.repository.split_once('/').map(|(owner, _)| owner))
        .collect();
    for owner in owners {
        for repository in catalog::organization_repositories(owner)? {
            if let Some(full) = repository["nameWithOwner"].as_str() {
                existing.insert(full.to_owned());
            }
        }
    }
    let product = &request.product.id;
    for repository in claims.keys().chain(existing.iter()) {
        if name(repository) == product.as_str() && !own.contains(repository.as_str()) {
            let claimed = claims.get(repository).map_or_else(
                || "no catalog product claims it yet".to_owned(),
                |owner| format!("catalog product {owner} claims it"),
            );
            bail!(
                "product {product} is not new: repository {repository} already exists and {claimed}; \
                 extend that product, or register the existing repository with \
                 `stado product registry add`, instead of creating it again"
            );
        }
    }
    for row in &request.repositories {
        let repository = row.repository.as_str();
        if own.contains(repository) {
            continue;
        }
        if let Some(owner) = claims.get(repository) {
            bail!(
                "repository {repository} already belongs to catalog product {owner}; \
                 a creation request provisions new repositories only, so extend {owner} instead"
            );
        }
        if existing.contains(repository) {
            bail!(
                "repository {repository} already exists and no catalog product claims it \
                 (`stado product catalog --unclaimed {}` lists it); register it with \
                 `stado product registry add` instead of creating it again",
                repository
                    .split_once('/')
                    .map_or(repository, |(owner, _)| owner)
            );
        }
    }
    Ok(())
}
