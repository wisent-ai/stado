//! Cleaner scan scopes are not deletion eligibility or recoverable capacity.

use serde_json::Value;

use super::paths;
use crate::providers::local::disk_cleanup::rule;

pub(super) struct Scope {
    pub cleaner: &'static rule::Cleaner,
    pub root: String,
}

/// Every cleaner's area on this host, resolved from the same host's reading,
/// never from this CLI's OS. The build-cache cleaner's areas are the tagged
/// trees the host measured, one scope each: it walks the whole home but takes
/// only those. Cleaners whose area is not a directory (the harness logs,
/// Time Machine's snapshots) have no scope.
pub(super) fn scopes(
    home: &str,
    platform: &str,
    report: &Value,
    weles_recordings_dir: Option<&str>,
) -> Vec<Scope> {
    let mut scopes = Vec::new();
    for cleaner in rule::CLEANERS {
        if cleaner.name == "build_caches" {
            let tagged = report["tagged_build_output"]
                .as_array()
                .into_iter()
                .flatten();
            scopes.extend(tagged.filter_map(|row| {
                Some(Scope {
                    cleaner,
                    root: row["path"].as_str()?.to_string(),
                })
            }));
            continue;
        }
        let root = if cleaner.name == "chromium_clones" {
            if !platform.starts_with("darwin-") {
                continue;
            }
            match report.get("chromium_clone_root").and_then(Value::as_str) {
                Some(root) => root.to_string(),
                None => continue,
            }
        } else if cleaner.name == crate::providers::local::disk_cleanup::weles::CLEANER {
            match weles_recordings_dir {
                Some(declared) => paths::absolute(declared, home),
                None => paths::absolute(&format!("~/{}", cleaner.root), home),
            }
        } else if cleaner.root.is_empty() {
            continue;
        } else {
            paths::absolute(&format!("~/{}", cleaner.root), home)
        };
        scopes.push(Scope { cleaner, root });
    }
    scopes
}

/// Only containment qualifies. A cleaner nested inside a parent does not own
/// that parent's siblings; the inventory partition reports them separately.
/// The innermost area wins.
pub(super) fn reach<'a>(path: &str, scopes: &'a [Scope]) -> Option<&'a Scope> {
    scopes
        .iter()
        .filter(|scope| paths::within(path, &scope.root))
        .max_by_key(|scope| scope.root.len())
}
