//! Which advisories of the database affect the packages each selected
//! checkout's committed lockfiles resolve.

pub mod lock;
pub mod parsed;

use super::osv::Database;
use anyhow::Result;
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf};

/// One checkout the command was asked to read, and what it found there.
#[derive(Clone, Serialize)]
pub struct Checkout {
    pub checkout: PathBuf,
    /// The catalog repository it holds, when a product named it.
    pub repository: Option<String>,
    /// Whether the checkout exists; an absent one leaves its exposure unknown.
    pub read: bool,
    /// The lockfiles its last commit holds.
    pub lockfiles: Vec<String>,
    /// The lockfiles among them whose working file differs from the last
    /// commit: rewritten, staged or deleted and not committed yet.
    pub uncommitted: Vec<String>,
}

/// One advisory that affects one package version one committed lockfile
/// resolves.
#[derive(Clone, Serialize)]
pub struct Finding {
    pub checkout: PathBuf,
    pub lockfile: String,
    pub package: String,
    pub version: String,
    pub advisory: String,
    pub aliases: Vec<String>,
    pub summary: Option<String>,
    pub informational: Option<String>,
    pub fixed: Vec<String>,
    pub compatible_fix: Option<String>,
    /// The packages of the same lockfile that depend on this version: whose
    /// requirement must move when no compatible fix exists.
    pub dependents: Vec<String>,
    /// When the lockfile's working file differs from the last commit, the
    /// versions of this package it locks there; empty when it locks none or
    /// the working tree deleted it.
    pub working: Option<Vec<String>>,
}

impl Finding {
    /// A vulnerability rather than an informational notice.
    pub fn actionable(&self) -> bool {
        self.informational.is_none()
    }

    /// Whether this finding is one of the advisories `wanted` names, by id or
    /// alias; every finding is when `wanted` is empty.
    pub fn named_by(&self, wanted: &[String]) -> bool {
        wanted.is_empty()
            || wanted
                .iter()
                .any(|id| self.advisory == *id || self.aliases.contains(id))
    }
}

/// Find each checkout's committed lockfiles and the ones whose working file
/// differs from the last commit, recording both on the checkout.
pub fn lockfiles(checkouts: &mut [Checkout]) -> Result<()> {
    for checkout in checkouts.iter_mut().filter(|checkout| checkout.read) {
        let lockfiles = lock::tracked(&checkout.checkout)?;
        let mut uncommitted = Vec::new();
        for lockfile in &lockfiles {
            if lock::changed(&checkout.checkout, lockfile)?.is_some() {
                uncommitted.push(lockfile.clone());
            }
        }
        checkout.lockfiles = lockfiles;
        checkout.uncommitted = uncommitted;
    }
    Ok(())
}

/// The crates.io versions `lockfile`'s working file locks, by package name;
/// none when the working tree deleted it.
fn working_versions(checkout: &Checkout, lockfile: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(text) = lock::working(&checkout.checkout, lockfile)? else {
        return Ok(by_name);
    };
    let label = checkout.checkout.join(lockfile).display().to_string();
    for package in parsed::packages(&label, &text)? {
        by_name
            .entry(package.name)
            .or_default()
            .push(package.version);
    }
    Ok(by_name)
}

/// Every finding in `checkouts`' committed lockfiles that `wanted` names, in
/// checkout, lockfile, package and advisory order.
pub fn findings(
    database: &Database,
    checkouts: &[Checkout],
    wanted: &[String],
) -> Result<Vec<Finding>> {
    let mut found = Vec::new();
    for checkout in checkouts {
        for lockfile in &checkout.lockfiles {
            let label = format!(
                "{} as last committed",
                checkout.checkout.join(lockfile).display()
            );
            let committed = lock::committed(&checkout.checkout, lockfile)?;
            let working = match checkout.uncommitted.contains(lockfile) {
                true => Some(working_versions(checkout, lockfile)?),
                false => None,
            };
            for package in parsed::packages(&label, &committed)? {
                for hit in database.affecting(&package.name, &package.version)? {
                    let finding = Finding {
                        checkout: checkout.checkout.clone(),
                        lockfile: lockfile.clone(),
                        package: package.name.clone(),
                        version: package.version.clone(),
                        advisory: hit.advisory,
                        aliases: hit.aliases,
                        summary: hit.summary,
                        informational: hit.informational,
                        fixed: hit.fixed,
                        compatible_fix: hit.compatible_fix,
                        dependents: package.dependents.clone(),
                        working: working.as_ref().map(|by_name| {
                            by_name
                                .get(&package.name)
                                .into_iter()
                                .flatten()
                                .cloned()
                                .collect()
                        }),
                    };
                    if finding.named_by(wanted) {
                        found.push(finding);
                    }
                }
            }
        }
    }
    Ok(found)
}
