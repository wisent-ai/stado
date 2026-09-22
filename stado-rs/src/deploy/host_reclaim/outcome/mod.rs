//! What the host measured, folded out of the marker lines it printed.
//!
//! One stage's record is [`Stage`], one run's is [`Reclamation`], and the
//! `--json` shape the operator console consumes is built from them in
//! [`to_report`].

use serde_json::{json, Value};

use crate::deploy::host_channel;
use crate::deploy::host_state::cleanup::cleaner_plans;

use super::{APPLY_MODE, DRY_RUN_MODE};

mod janitor;
mod report;
mod shape;

pub use report::to_report;
pub use shape::{Reclamation, Stage};
use janitor::janitor_sentence;
use shape::{blocks, drain, drain_evidence, unavailable};

const REGISTRY_CLEANUP_STAGE: &str = "registry_cleanup";

/// Fold the marker lines of stdout into a reclamation.
///
/// Item lines always precede the stage marker that closes their stage, because
/// each stage prints its own totals after its loop, so items are attached to
/// the stage that names them without buffering the whole stream.
pub fn parse_output(stdout: &str, apply: bool) -> Reclamation {
    let mut reclamation = Reclamation {
        mode: if apply { APPLY_MODE } else { DRY_RUN_MODE }.to_string(),
        ..Reclamation::default()
    };
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut refused: Vec<(String, String)> = Vec::new();
    let mut local_evidence: Vec<(String, Value)> = Vec::new();
    // Item lines are drained by the stage marker that closes them, through a
    // free function so the pending list stays borrowable by the arm that fills
    // it.
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_RECLAIM_FREE", "before", free] => reclamation.free_kb_before = blocks(free),
            ["STADO_RECLAIM_FREE", "after", free] => reclamation.free_kb_after = blocks(free),
            ["STADO_RECLAIM_ITEM", stage, path] => {
                pending.push(((*stage).to_string(), (*path).to_string()));
            }
            ["STADO_RECLAIM_REFUSED", stage, item, detail] => {
                refused.push(((*stage).to_string(), format!("{item}: {detail}")));
            }
            ["STADO_RECLAIM_LOCAL_EVIDENCE", stage, job_id, path, decision, process_absent, lease_expired, tree_age_seconds, absence_age_seconds] =>
            {
                local_evidence.push((
                    (*stage).to_string(),
                    json!({
                        "source": "local_observation",
                        "job_id": job_id,
                        "path": path,
                        "decision": decision,
                        "process_absent": *process_absent == "true",
                        "lease_expired": *lease_expired == "true",
                        "tree_age_seconds": blocks(tree_age_seconds),
                        "absence_age_seconds": blocks(absence_age_seconds),
                    }),
                ));
            }
            ["STADO_RECLAIM_STAGE", stage, before, after] => {
                let paths = drain(&mut pending, stage);
                let stage_refused = drain(&mut refused, stage);
                let stage_local_evidence = drain_evidence(&mut local_evidence, stage);
                let unavailable_name = format!("{stage}{UNAVAILABLE_SUFFIX}");
                if reclamation
                    .stages
                    .iter()
                    .any(|record| record.stage == unavailable_name)
                {
                    continue;
                }
                reclamation.stages.push(Stage {
                    stage: (*stage).to_string(),
                    free_kb_before: blocks(before),
                    free_kb_after: blocks(after),
                    items: paths.len(),
                    paths,
                    detail: None,
                    refused: stage_refused,
                    local_terminality_evidence: stage_local_evidence,
                });
            }
            ["STADO_RECLAIM_CLEANUP", before, after, plan] => {
                // The janitor's own numbers, never recounted here: what a pass
                // would remove in preview mode, what it did remove in apply
                // mode. A report that will not parse is a stage that did not
                // run, not a stage that freed nothing.
                let parsed: Option<Value> = serde_json::from_str(plan).ok();
                let Some(parsed) = parsed else {
                    reclamation.stages.push(unavailable(
                        REGISTRY_CLEANUP_STAGE,
                        "the host janitor produced no parseable report",
                    ));
                    continue;
                };
                if parsed.get("outcome").and_then(Value::as_str)
                    == Some("invalid_or_unavailable_policy")
                {
                    let detail = parsed
                        .get("errors")
                        .and_then(Value::as_array)
                        .map(|errors| {
                            errors
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                        .filter(|detail| !detail.is_empty())
                        .unwrap_or_else(|| {
                            "host janitor reported invalid_or_unavailable_policy".to_string()
                        });
                    reclamation.janitor_plan = Some(parsed);
                    reclamation
                        .stages
                        .push(unavailable(REGISTRY_CLEANUP_STAGE, &detail));
                    continue;
                }
                let plans = cleaner_plans(&parsed);
                let counted: i64 = plans
                    .iter()
                    .map(|cleaner| {
                        if apply {
                            cleaner.deleted_items
                        } else {
                            cleaner.eligible_items
                        }
                    })
                    .sum();
                let detail = janitor_sentence(&parsed, &plans, apply);
                reclamation.janitor_plan = Some(parsed);
                reclamation.stages.push(Stage {
                    stage: REGISTRY_CLEANUP_STAGE.to_string(),
                    free_kb_before: blocks(before),
                    free_kb_after: blocks(after),
                    items: usize::try_from(counted).unwrap_or_default(),
                    paths: Vec::new(),
                    detail: Some(detail),
                    refused: Vec::new(),
                    local_terminality_evidence: Vec::new(),
                });
            }
            ["STADO_RECLAIM_UNAVAILABLE", stage, detail] => {
                reclamation.stages.push(unavailable(stage, detail));
            }
            _ => {}
        }
    }
    reclamation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_terminality_evidence_is_machine_readable() {
        let report = parse_output(
            concat!(
                "STADO_RECLAIM_LOCAL_EVIDENCE\tqueue_workdirs\tjob-a\t/tmp/wc-job-a\t",
                "reclaimed\ttrue\ttrue\t1801\t901\n",
                "STADO_RECLAIM_ITEM\tqueue_workdirs\t/tmp/wc-job-a\n",
                "STADO_RECLAIM_STAGE\tqueue_workdirs\t100\t200\n",
            ),
            true,
        );
        let evidence = &report.stages[0].local_terminality_evidence[0];

        assert_eq!(evidence["source"], "local_observation");
        assert_eq!(evidence["job_id"], "job-a");
        assert_eq!(evidence["decision"], "reclaimed");
        assert_eq!(evidence["process_absent"], true);
        assert_eq!(evidence["lease_expired"], true);
        assert_eq!(evidence["tree_age_seconds"], 1801);
        assert_eq!(evidence["absence_age_seconds"], 901);
    }
}
