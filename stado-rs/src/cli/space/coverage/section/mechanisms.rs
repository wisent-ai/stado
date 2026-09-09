//! Cleaner scan scopes are not deletion eligibility or recoverable capacity.

use serde_json::Value;

use super::paths::{self, Occupant};
use crate::providers::local::disk_cleanup::catalogue;

#[derive(Debug, Clone)]
pub struct DeclaredCleaner {
    pub name: String,
    pub root: Option<String>,
}

pub(super) struct Scope {
    pub cleaner: &'static catalogue::CleanerDeclaration,
    pub root: String,
    pub declared: bool,
}

/// Resolve defaults from the same host's reading, never from this CLI's OS.
pub(super) fn scopes(
    home: &str,
    platform: &str,
    declared: &[DeclaredCleaner],
    report: &Value,
) -> Vec<Scope> {
    catalogue::CLEANERS.iter().filter_map(|cleaner| {
        let policy = declared.iter().find(|row| row.name == cleaner.name);
        let root = if let Some(root) = policy.and_then(|row| row.root.as_deref()) {
            paths::absolute(root, home)
        } else if cleaner.name == "chromium_clones" {
            if !platform.starts_with("darwin-") { return None; }
            report.get("chromium_clone_root")?.as_str()?.to_string()
        } else if cleaner.name == "build_caches" {
            report["build_caches"]["declaration"]["root"].as_str()?.to_string()
        } else {
            paths::absolute(&format!("~/{}", cleaner.default_root), home)
        };
        Some(Scope { cleaner, root, declared: policy.is_some() })
    }).collect()
}

/// Only containment qualifies. A cleaner nested inside a parent does not own
/// that parent's siblings; the inventory partition reports them separately.
pub(super) fn reach<'a>(path: &str, scopes: &'a [Scope]) -> Option<&'a Scope> {
    scopes.iter()
        .filter(|scope| paths::within(path, &scope.root))
        .max_by_key(|scope| (scope.declared, scope.root.len()))
}

pub(super) struct Unarmed<'a> {
    pub scope: &'a Scope,
    pub bytes: i64,
}

impl Unarmed<'_> {
    pub fn detail(&self) -> String {
        format!(
            "{} is not declared for this host; {} is its scan root. Check availability with `stado space cleaners list <target>` before declaring it; directory size is not removable size",
            self.scope.cleaner.name, self.scope.root
        )
    }
}

pub(super) fn unarmed<'a>(occupants: &[Occupant], scopes: &'a [Scope]) -> Vec<Unarmed<'a>> {
    let mut rows: Vec<_> = scopes.iter().filter(|scope| !scope.declared)
        .filter_map(|scope| {
            let bytes = paths::measured(&scope.root, occupants)?;
            (bytes > 0).then_some(Unarmed { scope, bytes })
        }).collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.bytes));
    rows
}
