//! Join measured directory scopes with the last recorded cleanup operation.

use serde_json::{json, Value};

use super::{paths, UNCOVERED_ROWS};
use crate::deploy::host_reclaim::StageDeclaration;
use mechanisms::DeclaredCleaner;

pub(super) mod mechanisms;
mod verdict;

pub fn section(
    report: &Value,
    stages: &[StageDeclaration],
    home: &str,
    platform: &str,
    free_space: &Value,
    declared_cleaners: &[DeclaredCleaner],
) -> Value {
    let available = free_space["available_bytes"].as_i64();
    let distance = |key: &str| available.zip(free_space[key].as_i64())
        .map(|(free, mark)| mark.saturating_sub(free).max(0));
    let need = distance("target_watermark_bytes");
    let deficit = distance("low_watermark_bytes");
    let occupants = paths::occupants(report);
    let covered = paths::covered(stages, home, platform, &occupants);
    let scopes = mechanisms::scopes(home, platform, declared_cleaners, report);
    let mut boundaries: Vec<String> = covered.iter().map(|row| row.root.clone()).collect();
    boundaries.extend(scopes.iter().map(|scope| scope.root.clone()));
    let partition = paths::partition(&occupants, &boundaries);
    let in_stage = |path: &str| covered.iter().any(|root| paths::within(path, &root.root));
    let stage_bytes = partition.iter().filter(|row| in_stage(&row.path))
        .fold(0_i64, |sum, row| sum.saturating_add(row.bytes));
    let outside: Vec<_> = partition.iter().filter(|row| !in_stage(&row.path)).collect();
    let outside_bytes = outside.iter().fold(0_i64, |sum, row| sum.saturating_add(row.bytes));
    let cleaner_bytes = outside.iter().filter(|row| {
        mechanisms::reach(&row.path, &scopes).is_some_and(|scope| scope.declared)
    }).fold(0_i64, |sum, row| sum.saturating_add(row.bytes));
    let unswept_bytes = outside_bytes.saturating_sub(cleaner_bytes);
    let word = verdict::verdict(deficit, !occupants.is_empty(), unswept_bytes);
    let unarmed = mechanisms::unarmed(&occupants, &scopes);
    let state = &report["cleanup_state"];
    json!({
        "need_bytes": need,
        "deficit_bytes": deficit,
        "covered": covered.iter().map(|row| json!({
            "stage": row.stage,
            "root": row.root,
            "bytes": row.bytes,
            "measured": row.bytes.is_some(),
        })).collect::<Vec<_>>(),
        "covered_bytes": stage_bytes,
        "uncovered": outside.iter().take(UNCOVERED_ROWS).map(|row| {
            let scope = mechanisms::reach(&row.path, &scopes);
            json!({
                "path": row.path,
                "bytes": row.bytes,
                "exclusive_of_measured_children": row.exclusive,
                "mechanism": scope.map(|scope| scope.cleaner.name),
                "mechanism_declared": scope.is_some_and(|scope| scope.declared),
            })
        }).collect::<Vec<_>>(),
        "uncovered_rows": outside.len(),
        "uncovered_bytes": outside_bytes,
        "cleaner_bytes": cleaner_bytes,
        "unswept_bytes": unswept_bytes,
        "reclaimable_bytes": Value::Null,
        "inventory_incomplete": report.get("inventory_incomplete"),
        "unarmed": unarmed.iter().map(|row| json!({
            "cleaner": row.scope.cleaner.name,
            "root": row.scope.root,
            "bytes": row.bytes,
            "since": row.scope.cleaner.since,
            "detail": row.detail(),
        })).collect::<Vec<_>>(),
        "verdict": word,
        "detail": verdict::detail(word, need, stage_bytes.saturating_add(cleaner_bytes), unswept_bytes),
        "janitor": {
            "outcome": state.get("outcome").and_then(Value::as_str).unwrap_or("never_run"),
            "detail": verdict::janitor_detail(state, need),
            "report": state.get("report"),
        },
        "roots_from": stages.iter().filter_map(|stage| stage.roots_from.as_ref()
            .map(|source| json!({"stage": stage.name, "source": source}))).collect::<Vec<_>>(),
    })
}
