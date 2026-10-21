//! `advisories apply`: move each affected package of a committed lockfile to
//! the first fixed version Cargo may take without a manifest change, with
//! `cargo update --precise`, and say why for every one it cannot move.

use super::scan::{lock, Finding};
use crate::{
    common::{checked, toolchain_command},
    source,
};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

/// What `apply` did for one affected package version of one lockfile.
#[derive(Serialize)]
pub struct Update {
    pub checkout: PathBuf,
    pub lockfile: String,
    pub package: String,
    pub from: String,
    /// The version Cargo was asked to lock, when one is compatible.
    pub to: Option<String>,
    pub advisories: Vec<String>,
    /// `updated`, `unfixed` (no compatible fix: a manifest requirement must
    /// move), `refused` (the lockfile could not be rewritten safely) or
    /// `failed` (Cargo refused the update).
    pub state: &'static str,
    pub cause: Option<String>,
}

/// The findings of one package version in one lockfile, keyed for grouping.
type Key = (PathBuf, String, String, String);

/// Why `lockfile` cannot be rewritten now, when it cannot.
fn refusal(checkout: &std::path::Path, lockfile: &str) -> Result<Option<String>> {
    let directory = lock::directory(checkout, lockfile)?;
    let manifest = directory.join(lock::MANIFEST);
    if !crate::common::files::present(&manifest)? {
        return Ok(Some(format!(
            "no {} beside {}: cargo update resolves a lockfile from the manifest beside it",
            lock::MANIFEST,
            checkout.join(lockfile).display()
        )));
    }
    let changes = source::git(checkout, &["status", "--porcelain", "--", lockfile])?;
    if !changes.is_empty() {
        return Ok(Some(format!(
            "{} has uncommitted changes ({changes}); commit or discard them first, so this \
             update is the only change to it",
            checkout.join(lockfile).display()
        )));
    }
    Ok(None)
}

/// Run `cargo update --precise` for each affected package version that has a
/// compatible fix, and report every actionable one.
pub fn run(findings: &[Finding]) -> Result<Vec<Update>> {
    let mut grouped: BTreeMap<Key, Vec<&Finding>> = BTreeMap::new();
    for finding in findings.iter().filter(|finding| finding.actionable()) {
        let key = (
            finding.checkout.clone(),
            finding.lockfile.clone(),
            finding.package.clone(),
            finding.version.clone(),
        );
        grouped.entry(key).or_default().push(finding);
    }
    let mut refused: BTreeMap<(PathBuf, String), Option<String>> = BTreeMap::new();
    let mut updates = Vec::new();
    for ((checkout, lockfile, package, from), group) in grouped {
        let advisories: Vec<String> = group
            .iter()
            .map(|finding| finding.advisory.clone())
            .collect();
        let fixes: BTreeSet<semver::Version> = group
            .iter()
            .filter_map(|finding| finding.compatible_fix.as_deref())
            .map(semver::Version::parse)
            .collect::<Result<_, _>>()?;
        let mut update = Update {
            checkout: checkout.clone(),
            lockfile: lockfile.clone(),
            package: package.clone(),
            from: from.clone(),
            to: fixes.last().map(ToString::to_string),
            advisories,
            state: "updated",
            cause: None,
        };
        let without_fix: Vec<&&Finding> = group
            .iter()
            .filter(|finding| finding.compatible_fix.is_none())
            .collect();
        if !without_fix.is_empty() {
            let fixed: BTreeSet<&str> = without_fix
                .iter()
                .flat_map(|finding| finding.fixed.iter().map(String::as_str))
                .collect();
            let dependents: BTreeSet<&str> = group
                .iter()
                .flat_map(|finding| finding.dependents.iter().map(String::as_str))
                .collect();
            update.state = "unfixed";
            update.cause = Some(format!(
                "no fixed version of {package} is caret-compatible with {from} (fixed in: {}); \
                 the requirement of {} on it must move in their manifests",
                listed(fixed.into_iter()),
                listed(dependents.into_iter()),
            ));
            updates.push(update);
            continue;
        }
        let key = (checkout.clone(), lockfile.clone());
        if !refused.contains_key(&key) {
            refused.insert(key.clone(), refusal(&checkout, &lockfile)?);
        }
        if let Some(cause) = refused.get(&key).cloned().flatten() {
            update.state = "refused";
            update.cause = Some(cause);
            updates.push(update);
            continue;
        }
        let Some(to) = update.to.clone() else {
            anyhow::bail!("{package} {from} has neither a compatible fix nor a reason it has none");
        };
        let directory = lock::directory(&checkout, &lockfile)?;
        let mut cargo = toolchain_command("cargo")?;
        cargo
            .arg("update")
            .arg("--manifest-path")
            .arg(directory.join(lock::MANIFEST))
            .arg("--package")
            .arg(format!("{package}@{from}"))
            .arg("--precise")
            .arg(&to)
            .current_dir(&directory);
        if let Err(error) = checked(&mut cargo) {
            update.state = "failed";
            update.cause = Some(format!(
                "cargo update refused {package} {from} -> {to}: {error:#}"
            ));
        }
        updates.push(update);
    }
    Ok(updates)
}

/// `items` joined for a sentence, or `none` when there are none.
fn listed<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let items: Vec<&str> = items.collect();
    match items.is_empty() {
        true => "none".to_owned(),
        false => items.join(", "),
    }
}
