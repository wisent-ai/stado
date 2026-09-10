//! What the host measured, folded out of the marker lines it printed.
//!
//! One stage's record is [`Stage`], one run's is [`Reclamation`], and the
//! `--json` shape the operator console consumes is built from them in
//! [`to_report`].

use serde_json::{json, Value};

use crate::deploy::host_channel;
use crate::deploy::host_state::cleanup::cleaner_plans;

use super::{APPLY_MODE, DRY_RUN_MODE, UNAVAILABLE_SUFFIX};

mod report;

pub use report::to_report;

const REGISTRY_CLEANUP_STAGE: &str = "registry_cleanup";

/// One stage, as the host measured it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stage {
    pub stage: String,
    /// `df -Pk` available blocks either side of the stage, or `None` for a
    /// stage that never ran.
    pub free_kb_before: Option<i64>,
    pub free_kb_after: Option<i64>,
    /// How many items the stage reclaimed, or in dry-run mode would reclaim.
    pub items: usize,
    /// The paths behind that count, when the stage produced them. The janitor
    /// reports per-cleaner counts and not paths, so this is empty for that
    /// stage rather than filled with placeholders.
    pub paths: Vec<String>,
    /// Why the stage could not run, for the host's own words in the rendering.
    pub detail: Option<String>,
    /// Snapshot identifiers refused by the native ownership/type checks.
    pub refused: Vec<String>,
    /// Per-workdir proof used only when the queue authority was unavailable.
    pub local_terminality_evidence: Vec<Value>,
}

/// Everything one reclamation did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reclamation {
    pub mode: String,
    pub stages: Vec<Stage>,
    pub free_kb_before: Option<i64>,
    pub free_kb_after: Option<i64>,
    /// The janitor's canonical report, kept for the per-cleaner rendering.
    pub janitor_plan: Option<Value>,
    /// Stages that did not run, and why. A stage nobody could judge must say
    /// so; reporting it as a stage that removed nothing is the same sentence
    /// as a clean host.
    pub skipped: Vec<(String, String)>,
}

/// A marker field that has to be a `df` block count.
fn blocks(field: &str) -> Option<i64> {
    field.trim().parse::<i64>().ok()
}

/// Take every pending item line that names `stage`, leaving the rest.
fn drain(pending: &mut Vec<(String, String)>, stage: &str) -> Vec<String> {
    let mut mine = Vec::new();
    pending.retain(|(named, path)| {
        if named == stage {
            mine.push(path.clone());
            return false;
        }
        true
    });
    mine
}

fn drain_evidence(pending: &mut Vec<(String, Value)>, stage: &str) -> Vec<Value> {
    let mut mine = Vec::new();
    pending.retain(|(named, evidence)| {
        if named == stage {
            mine.push(evidence.clone());
            return false;
        }
        true
    });
    mine
}

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
                let counted: i64 = cleaner_plans(&parsed)
                    .iter()
                    .map(|cleaner| {
                        if apply {
                            cleaner.deleted_items
                        } else {
                            cleaner.eligible_items
                        }
                    })
                    .sum();
                reclamation.janitor_plan = Some(parsed);
                reclamation.stages.push(Stage {
                    stage: REGISTRY_CLEANUP_STAGE.to_string(),
                    free_kb_before: blocks(before),
                    free_kb_after: blocks(after),
                    items: usize::try_from(counted).unwrap_or_default(),
                    paths: Vec::new(),
                    detail: None,
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

/// A stage the host could not run, named as itself.
fn unavailable(stage: &str, detail: &str) -> Stage {
    Stage {
        stage: format!("{stage}{UNAVAILABLE_SUFFIX}"),
        free_kb_before: None,
        free_kb_after: None,
        items: 0,
        paths: Vec::new(),
        detail: Some(detail.to_string()),
        refused: Vec::new(),
        local_terminality_evidence: Vec::new(),
    }
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
