//! Which measured paths a declared root covers, and which nothing covers.
//!
//! Only path arithmetic and the `du` rows the report already carries: no host
//! is read here and no second measurement is taken. The rules that matter are
//! the two that keep the arithmetic honest — a `du` walk reports a parent and
//! its children, so nothing is ever counted twice, and a parent that CONTAINS
//! a declared root is never reported as uncovered, because saying `~/.stado`
//! is unreachable would be false about the bytes `~/.stado/services` reaches.

use serde_json::Value;

use crate::deploy::host_reclaim::StageDeclaration;

/// One declared root, and what the inventory measured for it.
pub(super) struct Covered {
    pub(super) stage: String,
    pub(super) root: String,
    pub(super) bytes: Option<i64>,
}

/// One measured path, as the inventory reports it.
pub(super) struct Occupant {
    pub(super) path: String,
    pub(super) bytes: i64,
}

/// Every `du` row the host reported, as absolute path and bytes, largest first.
pub(super) fn occupants(report: &Value) -> Vec<Occupant> {
    let Some(rows) = report.get("inventory").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut occupants: Vec<Occupant> = rows
        .iter()
        .filter_map(|row| {
            let path = row.get("path").and_then(Value::as_str)?.to_string();
            // The inventory reports gibibytes, rounded to one decimal, because
            // that is the figure the report already published for every row.
            // Reading `du`'s kibibytes instead would mean a second host read
            // for numbers this report has already taken.
            let size_gb = row.get("size_gb").and_then(Value::as_f64)?;
            let bytes = (size_gb * 1024.0 * 1024.0 * 1024.0) as i64;
            Some(Occupant { path, bytes })
        })
        .collect();
    // The inventory walks several specs and two of them can reach one path:
    // the mini's home walk and its `local-storage` walk both report
    // `~/.stado/local-storage`, and the first coverage report listed that
    // directory twice as two findings of 52.4 GiB. One path is one occupant,
    // and the larger reading is the one kept.
    occupants.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(right.bytes.cmp(&left.bytes))
    });
    occupants.dedup_by(|later, kept| later.path == kept.path);
    occupants.sort_by_key(|row| std::cmp::Reverse(row.bytes));
    occupants
}

/// The declared root as an absolute path on the target.
fn absolute(root: &str, home: &str) -> String {
    match root.strip_prefix("~/") {
        Some(relative) => format!("{}/{relative}", home.trim_end_matches('/')),
        None => root.to_string(),
    }
}

/// Is `path` the root itself or something below it?
///
/// A declared root may carry a `*` in one segment, because several stages
/// sweep a shape rather than a directory: `/Users/Shared/*-runner/_work` is a
/// runner's work tree and `/Users/Shared` is not, and declaring the parent
/// would have told a coverage report that everything an operator keeps there
/// is reclaimable. The wildcard matches inside one segment only, never across
/// a `/`.
fn within(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    let mut expected = root.split('/');
    let mut actual = path.split('/');
    loop {
        match (expected.next(), actual.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(pattern), Some(segment)) if !segment_matches(pattern, segment) => return false,
            (Some(_), Some(_)) => {}
        }
    }
}

/// One path segment against one declared segment, with `*` standing for any
/// run of characters inside that segment.
fn segment_matches(pattern: &str, segment: &str) -> bool {
    let Some((head, tail)) = pattern.split_once('*') else {
        return pattern == segment;
    };
    if !segment.starts_with(head) {
        return false;
    }
    let rest = &segment[head.len()..];
    match tail.split_once('*') {
        // One wildcard: the remainder has to end with what follows it, and the
        // two halves must not overlap.
        None => rest.len() >= tail.len() && rest.ends_with(tail),
        // A declared root with two wildcards in one segment is not a shape any
        // stage sweeps; matching it loosely would be inventing coverage.
        Some(_) => false,
    }
}

/// The largest measurement the inventory carries for one root: the row for the
/// root itself when the walk reached it, otherwise the sum of the topmost rows
/// inside it.
///
/// Summed only over rows that do not contain one another, because a `du` walk
/// reports a parent and its children and adding both would double the root.
fn measured(root: &str, occupants: &[Occupant]) -> Option<i64> {
    if let Some(row) = occupants.iter().find(|row| row.path == root) {
        return Some(row.bytes);
    }
    let mut total: Option<i64> = None;
    let mut counted: Vec<&str> = Vec::new();
    for row in occupants.iter().filter(|row| within(&row.path, root)) {
        if counted.iter().any(|kept| within(&row.path, kept)) {
            continue;
        }
        counted.push(&row.path);
        total = Some(total.unwrap_or_default().saturating_add(row.bytes));
    }
    total
}

/// Every declared root this platform's stages sweep, expanded against the
/// target's own home.
pub(super) fn covered(
    stages: &[StageDeclaration],
    home: &str,
    platform: &str,
    occupants: &[Occupant],
) -> Vec<Covered> {
    stages
        .iter()
        // A stage whose own program refuses this operating system covers
        // nothing here, whatever its roots say. `foreign_home_trees` declares
        // `/Users`, which on a Mac is the entire home: counted as covered it
        // reported a full mini as fully reachable by a stage that begins by
        // refusing every host that is not Linux.
        .filter(|stage| {
            stage
                .os
                .as_deref()
                .is_none_or(|os| platform.split('-').next() == Some(os))
        })
        .flat_map(|stage| {
            stage.roots.iter().map(move |root| {
                let root = absolute(root, home);
                Covered {
                    stage: stage.name.clone(),
                    bytes: measured(&root, occupants),
                    root,
                }
            })
        })
        .collect()
}

/// The measured paths no declared root covers: the outermost ones, largest
/// first, capped at `limit` rows.
///
/// A path that contains a declared root is skipped for the reason in this
/// module's header; its children outside every root are what appear instead,
/// which is exactly how `local-storage` and `local-backup` surfaced while
/// `services` did not.
///
/// Containment is resolved before size, not while walking a size-ordered list.
/// A `du` walk reports `recordings` and `recordings/local` with the same
/// figure, and the first pass over the sorted rows kept whichever the sort
/// happened to put first, so an operator could be shown a subdirectory and its
/// parent as two findings worth the same bytes.
pub(super) fn uncovered(occupants: &[Occupant], roots: &[String], limit: usize) -> Vec<Occupant> {
    let candidates: Vec<&Occupant> = occupants
        .iter()
        .filter(|row| !roots.iter().any(|root| within(&row.path, root)))
        .filter(|row| !roots.iter().any(|root| within(root, &row.path)))
        .collect();
    let mut rows: Vec<Occupant> = candidates
        .iter()
        .filter(|row| {
            !candidates
                .iter()
                .any(|other| other.path != row.path && within(&row.path, &other.path))
        })
        .map(|row| Occupant {
            path: row.path.clone(),
            bytes: row.bytes,
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.bytes));
    rows.truncate(limit);
    rows
}
