//! The `--json` receipt one reclamation renders to.

use serde_json::{json, Map, Value};

use crate::deploy::host_disk::gib_from_blocks;
use crate::targets::ComputeTarget;

use super::Reclamation;

/// The reclamation as the `--json` report, in the exact shape the operator
/// console consumes.
pub fn to_report(target: &ComputeTarget, reclamation: &Reclamation) -> Map<String, Value> {
    let free = |blocks: Option<i64>| match blocks {
        Some(blocks) => json!(gib_from_blocks(blocks as f64)),
        None => Value::Null,
    };
    let mut report = Map::new();
    report.insert("host".to_string(), json!(target.name));
    report.insert("mode".to_string(), json!(reclamation.mode));
    report.insert(
        "stages".to_string(),
        Value::Array(
            reclamation
                .stages
                .iter()
                .map(|stage| {
                    json!({
                        "stage": stage.stage,
                        "free_gb_before": free(stage.free_kb_before),
                        "free_gb_after": free(stage.free_kb_after),
                        "items": stage.items,
                        "paths": stage.paths,
                        "refused": stage.refused,
                        "detail": stage.detail,
                        "local_terminality_evidence": stage.local_terminality_evidence,
                    })
                })
                .collect(),
        ),
    );
    // Named in the receipt, not only on the terminal: a consumer that reads
    // `stages` alone cannot tell a stage that ran and found nothing from a
    // stage nobody could judge.
    report.insert(
        "skipped".to_string(),
        Value::Array(
            reclamation
                .skipped
                .iter()
                .map(|(stage, reason)| json!({"stage": stage, "reason": reason}))
                .collect(),
        ),
    );
    report.insert(
        "free_gb_before".to_string(),
        free(reclamation.free_kb_before),
    );
    report.insert("free_gb_after".to_string(), free(reclamation.free_kb_after));
    report
}
