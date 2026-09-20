//! What the host holds in build output, and whether the declared cleaner
//! reaches it.
//!
//! A declaration is not a measurement. `lukasz-macbook` declared the
//! `build_caches` cleaner for months; its root resolved to the fleet's own
//! build cache, the operator's 843 GB of tagged `target/` trees sat four and
//! five levels down in a checkout, and every surface said the host was fine
//! while the volume stood at 97% and a release build failed for want of
//! scratch. Nothing was broken and nothing was missing: the cleaner covered
//! what it was pointed at, and nobody was told what it was not pointed at.
//!
//! This block is that sentence. It is computed from the census the disk
//! report already collects and the cleaner roots the registry already
//! declares, and it carries the root a declaration would need, so the repair
//! is one command rather than an investigation.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::mechanisms::Scope;
use super::paths;

/// One tagged tree the host measured.
struct Tagged {
    path: String,
    bytes: i64,
}

fn tagged(report: &Value) -> Vec<Tagged> {
    report["tagged_build_output"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(Tagged {
                        path: row["path"].as_str()?.to_string(),
                        bytes: row["bytes"].as_i64().unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The directory a declaration would have to name to reach every tree listed:
/// their deepest common ancestor.
///
/// Deliberately the ancestor rather than the longest row. A root that covers
/// one checkout and misses the three beside it is the state this block exists
/// to end, and the cleaner deletes only what carries a build tool's own tag,
/// so a wider root does not widen what may be removed.
fn common_ancestor(paths: &[&str]) -> Option<PathBuf> {
    let mut rows = paths.iter();
    let mut ancestor = PathBuf::from(rows.next()?);
    for path in rows {
        let candidate = Path::new(path);
        while !candidate.starts_with(&ancestor) {
            if !ancestor.pop() {
                return None;
            }
        }
    }
    (ancestor.components().count() > 1).then_some(ancestor)
}

/// The build-output block: what was measured, what the declared cleaners
/// reach, and the exact declaration that would close the gap.
pub(super) fn section(report: &Value, scopes: &[Scope], target: &str) -> Value {
    let rows = tagged(report);
    let reached = |path: &str| {
        scopes
            .iter()
            .any(|scope| scope.declared && paths::within(path, &scope.root))
    };
    let measured_bytes = rows
        .iter()
        .fold(0_i64, |sum, row| sum.saturating_add(row.bytes));
    let unreached: Vec<&Tagged> = rows.iter().filter(|row| !reached(&row.path)).collect();
    let unreached_bytes = unreached
        .iter()
        .fold(0_i64, |sum, row| sum.saturating_add(row.bytes));
    let names: Vec<&str> = unreached.iter().map(|row| row.path.as_str()).collect();
    let suggested = common_ancestor(&names);
    let remedy = suggested.as_ref().map(|root| {
        format!(
            "stado space cleaners declare {target} --cleaner build_caches --root {}",
            root.display()
        )
    });
    let read = report["tagged_build_output_read"].as_bool() == Some(true);
    let detail = match (read, unreached.is_empty(), &remedy) {
        (false, _, _) => {
            "build output was not measured on this host, so nothing here says whether a cleaner \
             reaches it"
                .to_string()
        }
        (true, true, _) => format!(
            "every measured build cache is inside a declared cleaner root ({} tree(s))",
            rows.len()
        ),
        (true, false, Some(remedy)) => format!(
            "{} of build output in {} tree(s) is outside every declared cleaner root; declare one with: {remedy}",
            super::super::render::gib(unreached_bytes),
            unreached.len()
        ),
        (true, false, None) => format!(
            "{} of build output in {} tree(s) is outside every declared cleaner root",
            super::super::render::gib(unreached_bytes),
            unreached.len()
        ),
    };
    json!({
        "measured_trees": rows.len(),
        "measured_bytes": measured_bytes,
        "unreached_trees": unreached.len(),
        "unreached_bytes": unreached_bytes,
        "suggested_root": suggested.map(|root| root.display().to_string()),
        "measured": read,
        "remedy": remedy,
        "detail": detail,
    })
}
