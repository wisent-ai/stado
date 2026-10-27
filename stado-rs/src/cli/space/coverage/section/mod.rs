//! Join measured directory scopes with the rule's verdict and the last
//! recorded cleanup operation.

use serde_json::{json, Value};

use super::paths;
use crate::deploy::host_reclaim::StageDeclaration;
use crate::providers::local::space_reclaim::rule::{self, VolumeReading};

pub(super) mod mechanisms;
mod verdict;

/// The host's volume as its own `df` read it (the rule's input), or why it
/// is unmeasured, through the one reader the host verdict also uses.
fn reading(report: &Value) -> (Option<VolumeReading>, Option<String>) {
    match crate::deploy::host_disk::volume_of_report(report) {
        Ok(volume) => (Some(volume), None),
        Err(cause) => (None, Some(cause)),
    }
}

pub fn section(
    report: &Value,
    stages: &[StageDeclaration],
    home: &str,
    platform: &str,
    weles_recordings_dir: Option<&str>,
) -> Value {
    let (volume, unmeasured) = reading(report);
    let occupants = paths::occupants(report);
    let covered = paths::covered(stages, home, platform, &occupants);
    let scopes = mechanisms::scopes(home, platform, report, weles_recordings_dir);
    let mut boundaries: Vec<String> = covered.iter().map(|row| row.root.clone()).collect();
    boundaries.extend(scopes.iter().map(|scope| scope.root.clone()));
    let partition = paths::partition(&occupants, &boundaries);
    let in_stage = |path: &str| covered.iter().any(|root| paths::within(path, &root.root));
    let stage_bytes: i64 = partition
        .iter()
        .filter(|row| in_stage(&row.path))
        .map(|row| row.bytes)
        .sum();
    let outside: Vec<_> = partition
        .iter()
        .filter(|row| !in_stage(&row.path))
        .collect();
    let outside_bytes: i64 = outside.iter().map(|row| row.bytes).sum();
    let cleaner_bytes: i64 = outside
        .iter()
        .filter(|row| mechanisms::reach(&row.path, &scopes).is_some())
        .map(|row| row.bytes)
        .sum();
    let unswept_bytes = outside_bytes.saturating_sub(cleaner_bytes);
    let word = verdict::verdict(volume);
    let state = &report["cleanup_state"];
    let incomplete = report.get("inventory_incomplete").filter(|why| !why.is_null());
    let complete = incomplete.is_none();
    json!({
        "rule": rule::rule_json(volume),
        "headroom_bytes": volume.map(|volume| volume.headroom_bytes()),
        "cleaner_scopes": scopes.iter().map(|scope| json!({
            "cleaner": scope.cleaner.name,
            "root": scope.root,
            "bytes": paths::measured(&scope.root, &occupants),
        })).collect::<Vec<_>>(),
        "covered": covered.iter().map(|row| json!({
            "stage": row.stage,
            "root": row.root,
            "bytes": row.bytes,
            "measured": row.bytes.is_some(),
        })).collect::<Vec<_>>(),
        "covered_bytes": complete.then_some(stage_bytes),
        "uncovered": outside.iter().map(|row| json!({
            "path": row.path,
            "bytes": row.bytes,
            "exclusive_of_measured_children": row.exclusive,
            "mechanism": mechanisms::reach(&row.path, &scopes).map(|scope| scope.cleaner.name),
        })).collect::<Vec<_>>(),
        "uncovered_rows": outside.len(),
        "uncovered_bytes": complete.then_some(outside_bytes),
        "cleaner_bytes": complete.then_some(cleaner_bytes),
        "unswept_bytes": complete.then_some(unswept_bytes),
        // Partial observations remain useful, but are not whole-inventory
        // totals and cannot prove that no unswept bytes exist.
        "measured_bytes": incomplete.map(|_| json!({
            "covered": stage_bytes,
            "uncovered": outside_bytes,
            "cleaner": cleaner_bytes,
            "unswept": unswept_bytes,
        })),
        "reclaimable_bytes": Value::Null,
        "inventory_incomplete": incomplete,
        "verdict": word,
        "detail": verdict::detail(
            word,
            volume,
            stage_bytes.saturating_add(cleaner_bytes),
            unswept_bytes,
            incomplete,
            unmeasured.as_deref(),
        ),
        "janitor": {
            "outcome": match state.get("outcome").and_then(Value::as_str) {
                Some(outcome) => outcome,
                None => "never_run",
            },
            "detail": verdict::janitor_detail(state),
            "report": state.get("report"),
        },
        "roots_from": stages.iter().filter_map(|stage| stage.roots_from.as_ref()
            .map(|source| json!({"stage": stage.name, "source": source}))).collect::<Vec<_>>(),
    })
}
