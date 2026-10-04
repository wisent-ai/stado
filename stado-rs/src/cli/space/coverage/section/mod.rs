//! Join measured directory scopes with the rule's verdict and the last
//! recorded cleanup operation.

use serde_json::{json, Value};

use super::{paths, UNCOVERED_ROWS};
use crate::deploy::host_reclaim::StageDeclaration;
use crate::providers::local::disk_cleanup::rule::{self, VolumeReading};

pub(super) mod mechanisms;
mod verdict;

/// The host's volume as its own `df` read it: the rule's input.
pub fn reading(report: &Value) -> Option<VolumeReading> {
    let kib = |key: &str| {
        report["usage"][key]
            .as_str()
            .and_then(|value| value.parse::<i64>().ok())
            .and_then(|value| value.checked_mul(1024))
    };
    Some(VolumeReading {
        total_bytes: kib("blocks_kb")?,
        free_bytes: kib("available_kb")?,
    })
}

pub fn section(
    report: &Value,
    stages: &[StageDeclaration],
    home: &str,
    platform: &str,
    weles_recordings_dir: Option<&str>,
) -> Value {
    let volume = reading(report);
    let occupants = paths::occupants(report);
    let covered = paths::covered(stages, home, platform, &occupants);
    let scopes = mechanisms::scopes(home, platform, report, weles_recordings_dir);
    let mut boundaries: Vec<String> = covered.iter().map(|row| row.root.clone()).collect();
    boundaries.extend(scopes.iter().map(|scope| scope.root.clone()));
    let partition = paths::partition(&occupants, &boundaries);
    let in_stage = |path: &str| covered.iter().any(|root| paths::within(path, &root.root));
    let stage_bytes = partition
        .iter()
        .filter(|row| in_stage(&row.path))
        .fold(0_i64, |sum, row| sum.saturating_add(row.bytes));
    let outside: Vec<_> = partition
        .iter()
        .filter(|row| !in_stage(&row.path))
        .collect();
    let outside_bytes = outside
        .iter()
        .fold(0_i64, |sum, row| sum.saturating_add(row.bytes));
    let cleaner_bytes = outside
        .iter()
        .filter(|row| mechanisms::reach(&row.path, &scopes).is_some())
        .fold(0_i64, |sum, row| sum.saturating_add(row.bytes));
    let unswept_bytes = outside_bytes.saturating_sub(cleaner_bytes);
    let word = verdict::verdict(volume);
    let state = &report["cleanup_state"];
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
        "covered_bytes": stage_bytes,
        "uncovered": outside.iter().take(UNCOVERED_ROWS).map(|row| json!({
            "path": row.path,
            "bytes": row.bytes,
            "exclusive_of_measured_children": row.exclusive,
            "mechanism": mechanisms::reach(&row.path, &scopes).map(|scope| scope.cleaner.name),
        })).collect::<Vec<_>>(),
        "uncovered_rows": outside.len(),
        "uncovered_bytes": outside_bytes,
        "cleaner_bytes": cleaner_bytes,
        "unswept_bytes": unswept_bytes,
        "reclaimable_bytes": Value::Null,
        "inventory_incomplete": report.get("inventory_incomplete"),
        "verdict": word,
        "detail": verdict::detail(word, volume, stage_bytes.saturating_add(cleaner_bytes), unswept_bytes),
        "janitor": {
            "outcome": state.get("outcome").and_then(Value::as_str).unwrap_or("never_run"),
            "detail": verdict::janitor_detail(state),
            "report": state.get("report"),
        },
        "roots_from": stages.iter().filter_map(|stage| stage.roots_from.as_ref()
            .map(|source| json!({"stage": stage.name, "source": source}))).collect::<Vec<_>>(),
    })
}
