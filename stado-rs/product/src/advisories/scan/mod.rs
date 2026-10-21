//! Which advisories of the database affect the packages each selected
//! checkout's committed lockfiles resolve.

pub mod lock;

use super::osv::Database;
use anyhow::Result;
use serde::Serialize;
use std::path::PathBuf;

/// One checkout the command was asked to read, and what it found there.
#[derive(Clone, Serialize)]
pub struct Checkout {
    pub checkout: PathBuf,
    /// The catalog repository it holds, when a product named it.
    pub repository: Option<String>,
    /// Whether the checkout exists; an absent one leaves its exposure unknown.
    pub read: bool,
    pub lockfiles: Vec<String>,
}

/// One advisory that affects one package version one lockfile resolves.
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

/// Find each checkout's tracked lockfiles, recording them on the checkout.
pub fn lockfiles(checkouts: &mut [Checkout]) -> Result<()> {
    for checkout in checkouts.iter_mut().filter(|checkout| checkout.read) {
        checkout.lockfiles = lock::tracked(&checkout.checkout)?;
    }
    Ok(())
}

/// Every finding in `checkouts`' lockfiles that `wanted` names, in checkout,
/// lockfile, package and advisory order.
pub fn findings(
    database: &Database,
    checkouts: &[Checkout],
    wanted: &[String],
) -> Result<Vec<Finding>> {
    let mut found = Vec::new();
    for checkout in checkouts {
        for lockfile in &checkout.lockfiles {
            let path = checkout.checkout.join(lockfile);
            for package in lock::packages(&path)? {
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
