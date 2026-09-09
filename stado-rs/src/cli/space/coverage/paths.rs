//! Attribute measured bytes without charging a parent to one covered child.
//! Nested inventory rows are partitioned only at declared scope boundaries;
//! a parent's remaining bytes are explicitly exclusive of its measured children.

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
    pub(super) exclusive: bool,
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
            let bytes = row.get("bytes").and_then(Value::as_i64)?;
            Some(Occupant {
                path,
                bytes,
                exclusive: false,
            })
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
pub(super) fn absolute(root: &str, home: &str) -> String {
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
pub(super) fn within(path: &str, root: &str) -> bool {
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

/// Sum non-overlapping measurements within one scope.
pub(super) fn measured(root: &str, occupants: &[Occupant]) -> Option<i64> {
    let mut rows: Vec<&Occupant> = occupants
        .iter()
        .filter(|row| within(&row.path, root))
        .collect();
    rows.sort_by(|left, right| left.path.cmp(&right.path));
    let mut total: Option<i64> = None;
    let mut previous: Option<&str> = None;
    for row in rows {
        if previous.is_some_and(|parent| within(&row.path, parent)) {
            continue;
        }
        previous = Some(&row.path);
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

/// Whether an observed parent contains a possibly wildcarded scope.
fn contains_scope(path: &str, scope: &str) -> bool {
    let mut pattern = scope.trim_end_matches('/').split('/');
    for segment in path.trim_end_matches('/').split('/') {
        if !pattern
            .next()
            .is_some_and(|expected| segment_matches(expected, segment))
        {
            return false;
        }
    }
    pattern.next().is_some()
}

/// Partition the complete inventory before applying any display limit.
pub(super) fn partition(occupants: &[Occupant], scopes: &[String]) -> Vec<Occupant> {
    let mut rows: Vec<&Occupant> = occupants.iter().collect();
    rows.sort_by(|left, right| left.path.cmp(&right.path));
    let mut children = vec![Vec::new(); rows.len()];
    let mut top = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|parent| !within(&row.path, &rows[*parent].path))
        {
            stack.pop();
        }
        if let Some(parent) = stack.last() {
            children[*parent].push(index);
        } else {
            top.push(index);
        }
        stack.push(index);
    }
    let mut output = Vec::new();
    for index in top {
        partition_node(index, &rows, &children, scopes, &mut output);
    }
    output.sort_by_key(|row| std::cmp::Reverse(row.bytes));
    output
}

fn partition_node(
    index: usize,
    rows: &[&Occupant],
    children: &[Vec<usize>],
    scopes: &[String],
    output: &mut Vec<Occupant>,
) {
    let row = rows[index];
    let split =
        !children[index].is_empty() && scopes.iter().any(|scope| contains_scope(&row.path, scope));
    if !split {
        output.push(Occupant {
            path: row.path.clone(),
            bytes: row.bytes,
            exclusive: false,
        });
        return;
    }
    let child_bytes = children[index]
        .iter()
        .fold(0_i64, |sum, child| sum.saturating_add(rows[*child].bytes));
    let remainder = row.bytes.saturating_sub(child_bytes).max(0);
    if remainder > 0 {
        output.push(Occupant {
            path: row.path.clone(),
            bytes: remainder,
            exclusive: true,
        });
    }
    for child in &children[index] {
        partition_node(*child, rows, children, scopes, output);
    }
}
