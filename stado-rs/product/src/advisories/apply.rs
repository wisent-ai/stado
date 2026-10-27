//! `advisories apply`: move each affected package of a committed lockfile to
//! the first fixed version Cargo may take without a manifest change, with
//! `cargo update --precise`, name every other package that update moved, and
//! say why for every advisory it cannot close.

use super::scan::{lock, parsed, Finding};
use crate::common::{checked, toolchain_command};
use anyhow::{bail, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// One other package an update moved while locking the fix: what the
/// lockfile resolved it to before and after, each as `version`, or as
/// `version source` for a package from anywhere but crates.io.
#[derive(Serialize)]
pub struct Moved {
    pub package: String,
    pub from: Vec<String>,
    pub to: Vec<String>,
}

/// What became of one affected package version.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Cargo locked the compatible fix.
    Updated,
    /// An earlier update of the same lockfile already moved this version,
    /// so Cargo was not asked again.
    Moved,
    /// No compatible fix: a manifest requirement must move.
    Unfixed,
    /// The lockfile could not be rewritten safely.
    Refused,
    /// Cargo refused the update.
    Failed,
}

impl State {
    /// Whether the lockfile no longer holds the version this update was
    /// for, by this update or an earlier one.
    pub fn made(self) -> bool {
        match self {
            State::Updated | State::Moved => true,
            State::Unfixed | State::Refused | State::Failed => false,
        }
    }

    /// The state as the answer names it.
    pub fn word(self) -> &'static str {
        match self {
            State::Updated => "updated",
            State::Moved => "moved",
            State::Unfixed => "unfixed",
            State::Refused => "refused",
            State::Failed => "failed",
        }
    }
}

/// What `apply` did for one affected package version of one lockfile, for
/// the advisories it names.
#[derive(Serialize)]
pub struct Update {
    pub checkout: PathBuf,
    pub lockfile: String,
    pub package: String,
    pub from: String,
    /// The version Cargo was asked to lock, when one is compatible.
    pub to: Option<String>,
    pub advisories: Vec<String>,
    pub state: State,
    pub cause: Option<String>,
    /// `updated` only: every other package whose version or source the
    /// update changed in the lockfile, since Cargo also moves what the fixed
    /// version requires.
    pub moved: Vec<Moved>,
}

/// The findings of one package version in one lockfile, keyed for grouping.
type Key = (PathBuf, String, String, String);

/// Why `lockfile` cannot be rewritten now, when it cannot.
fn refusal(checkout: &Path, lockfile: &str) -> Result<Option<String>> {
    let directory = lock::directory(checkout, lockfile)?;
    let manifest = directory.join(lock::MANIFEST);
    if !crate::common::files::present(&manifest)? {
        return Ok(Some(format!(
            "no {} beside {}: cargo update resolves a lockfile from the manifest beside it",
            lock::MANIFEST,
            checkout.join(lockfile).display()
        )));
    }
    Ok(lock::changed(checkout, lockfile)?.map(|changes| {
        format!(
            "{} has uncommitted changes ({changes}); commit or discard them first, so this \
             update is the only change to it",
            checkout.join(lockfile).display()
        )
    }))
}

/// Every package `lockfile`'s working file resolves now.
fn resolved_now(checkout: &Path, lockfile: &str) -> Result<parsed::Resolved> {
    let path = checkout.join(lockfile);
    let Some(text) = lock::working(checkout, lockfile)? else {
        bail!("{} is gone from the working tree", path.display());
    };
    parsed::resolved(&path.display().to_string(), &text)
}

/// Every package whose resolution differs between `before` and `after`,
/// leaving out the one move Cargo was asked for: `package` from `asked_from`
/// to `asked_to`.
fn moved(
    before: &parsed::Resolved,
    after: &parsed::Resolved,
    package: &str,
    asked_from: &str,
    asked_to: &str,
) -> Vec<Moved> {
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let mut moved = Vec::new();
    for name in names {
        let was: BTreeSet<&String> = before.get(name).into_iter().flatten().collect();
        let now: BTreeSet<&String> = after.get(name).into_iter().flatten().collect();
        let asked = name == package;
        let from: Vec<String> = was
            .difference(&now)
            .filter(|at| !(asked && at.as_str() == asked_from))
            .map(|at| (*at).clone())
            .collect();
        let to: Vec<String> = now
            .difference(&was)
            .filter(|at| !(asked && at.as_str() == asked_to))
            .map(|at| (*at).clone())
            .collect();
        if !from.is_empty() || !to.is_empty() {
            moved.push(Moved {
                package: name.clone(),
                from,
                to,
            });
        }
    }
    moved
}

/// The ids of `findings`' advisories.
fn ids(findings: &[&Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|finding| finding.advisory.clone())
        .collect()
}

/// The answer for the advisories of `package` at `from` that no fixed
/// version caret-compatible with it closes.
fn unfixed(key: &Key, findings: &[&Finding]) -> Update {
    let (checkout, lockfile, package, from) = key;
    let fixed: BTreeSet<&str> = findings
        .iter()
        .flat_map(|finding| finding.fixed.iter().map(String::as_str))
        .collect();
    let dependents: BTreeSet<&str> = findings
        .iter()
        .flat_map(|finding| finding.dependents.iter().map(String::as_str))
        .collect();
    Update {
        checkout: checkout.clone(),
        lockfile: lockfile.clone(),
        package: package.clone(),
        from: from.clone(),
        to: None,
        advisories: ids(findings),
        state: State::Unfixed,
        cause: Some(format!(
            "no fixed version of {package} is caret-compatible with {from} (fixed in: {}); \
             the requirement of {} on it must move in their manifests",
            listed(fixed.into_iter()),
            listed(dependents.into_iter()),
        )),
        moved: Vec::new(),
    }
}

/// Run `cargo update --precise` for each affected package version, to the
/// highest compatible fix its advisories name, and report every actionable
/// advisory: the ones that update closes, and apart from them the ones no
/// compatible version closes.
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
    for (key, group) in grouped {
        let (fixable, without_fix): (Vec<&Finding>, Vec<&Finding>) = group
            .into_iter()
            .partition(|finding| finding.compatible_fix.is_some());
        if !without_fix.is_empty() {
            updates.push(unfixed(&key, &without_fix));
        }
        let fixes: BTreeSet<semver::Version> = fixable
            .iter()
            .filter_map(|finding| finding.compatible_fix.as_deref())
            .map(semver::Version::parse)
            .collect::<Result<_, _>>()?;
        let Some(to) = fixes.last().map(ToString::to_string) else {
            continue;
        };
        let (checkout, lockfile, package, from) = key;
        let mut update = Update {
            checkout: checkout.clone(),
            lockfile: lockfile.clone(),
            package: package.clone(),
            from: from.clone(),
            to: Some(to.clone()),
            advisories: ids(&fixable),
            state: State::Updated,
            cause: None,
            moved: Vec::new(),
        };
        let held = (checkout.clone(), lockfile.clone());
        if !refused.contains_key(&held) {
            refused.insert(held.clone(), refusal(&checkout, &lockfile)?);
        }
        if let Some(cause) = refused.get(&held).cloned().flatten() {
            update.state = State::Refused;
            update.cause = Some(cause);
            updates.push(update);
            continue;
        }
        let before = resolved_now(&checkout, &lockfile)?;
        if !before
            .get(&package)
            .is_some_and(|held| held.contains(&from))
        {
            let held: Vec<&str> = before
                .get(&package)
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect();
            update.state = State::Moved;
            update.to = None;
            update.cause = Some(format!(
                "an earlier update of this lockfile already moved {package} {from}; the \
                 working lockfile now locks {}, and the findings say whether that closes {}",
                listed(held.into_iter()),
                update.advisories.join(", ")
            ));
            updates.push(update);
            continue;
        }
        let directory = lock::directory(&checkout, &lockfile)?;
        let mut cargo = toolchain_command("cargo")?;
        lock::unbound(&mut cargo)?;
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
            update.state = State::Failed;
            update.cause = Some(format!(
                "cargo update refused {package} {from} -> {to}: {error:#}"
            ));
            updates.push(update);
            continue;
        }
        let after = resolved_now(&checkout, &lockfile)?;
        update.moved = moved(&before, &after, &package, &from, &to);
        updates.push(update);
    }
    Ok(updates)
}

/// `items` joined for a sentence, or `none` when there are none.
pub fn listed<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let items: Vec<&str> = items.collect();
    match items.is_empty() {
        true => "none".to_owned(),
        false => items.join(", "),
    }
}
