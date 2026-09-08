//! The outcome pass: feedback and savings measurements for decisions whose
//! subject job has finished, bounded by a resumable cursor over the backlog.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autonomy::model::{DecisionKind, SavingsMeasurement, SCHEMA_VERSION};
use crate::autonomy::policy::AutonomyPolicy;
use crate::queue::{JobStorage, StorageError};

/// How many outstanding decisions one outcome pass downloads.
///
/// The cost bound on a set that does not drain: a decision whose subject job
/// never reaches a terminal prefix can never be given feedback, so it stays
/// outstanding for good, and every tick re-read the whole backlog.
const OUTSTANDING_PER_TICK: usize = 256;

/// Where the outcome pass records the last decision id it examined, so the
/// next one continues instead of repeating the front of the set.
const OUTCOME_CURSOR_PATH: &str = "state/autonomy/outcome-scan-cursor.json";

#[derive(Serialize, Deserialize)]
struct OutcomeCursor {
    /// The last decision id this pass looked at, in the set's own order.
    last_decision_id: String,
}

async fn read_outcome_cursor(store: &JobStorage) -> Result<Option<String>, StorageError> {
    let Some(raw) = store.download_text(OUTCOME_CURSOR_PATH).await? else {
        return Ok(None);
    };
    // A cursor that cannot be read is a cursor that has not been written yet:
    // the walk restarts at the head, which costs one pass and loses nothing.
    Ok(serde_json::from_str::<OutcomeCursor>(&raw)
        .ok()
        .map(|cursor| cursor.last_decision_id))
}

async fn write_outcome_cursor(store: &JobStorage, last: &str) -> Result<(), StorageError> {
    super::storage::write_json(
        store,
        OUTCOME_CURSOR_PATH,
        &OutcomeCursor {
            last_decision_id: last.to_string(),
        },
        false,
    )
    .await
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutcomeSummary {
    pub feedback_written: usize,
    pub savings_measured: usize,
}
/// Write the feedback and savings measurements that are still missing.
///
/// Only the work that is actually outstanding is read. Which decisions already
/// have feedback, and which savings already have a measurement, are answered by
/// the object names through one listing each; bodies are downloaded only for
/// the decisions that can still produce something.
///
/// The window matters as much as the filter. A decision whose job has already
/// left `completed/` and `failed/` can never produce feedback, so without a
/// bound it stays outstanding forever and is re-read on every tick. The bound
/// is the policy's own artifact retention: past it,
/// [`crate::autonomy::lifecycle`] would delete the record anyway, so
/// re-examining it is work the deployment has already declared worthless.
///
/// Before 2026-09-02 this function downloaded every decision, feedback, savings
/// and measurement record ever written and then read one or two job objects per
/// decision — 11,514 objects and roughly 16,000 more job reads per tick against
/// a store that also serves the public release channel. That is what held the
/// 0.13.42 release download at 570 KB/s until the deploy failed.
pub async fn measure_outcomes(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<OutcomeSummary, StorageError> {
    let mut summary = OutcomeSummary::default();
    let feedback_ids = super::storage::list_feedback_decision_ids(store).await?;
    let measured_ids = super::storage::list_measured_savings_ids(store).await?;
    let mut pending_savings = Vec::new();
    for savings_id in super::storage::list_savings_ids(store).await? {
        if measured_ids.contains(&savings_id) {
            continue;
        }
        if let Some(record) = super::storage::load_savings(store, &savings_id).await? {
            pending_savings.push(record);
        }
    }
    let artifact_seconds =
        i64::try_from(policy.idle.artifact_days * crate::monitor::billing::SECONDS_PER_DAY)
            .unwrap_or(i64::MAX);
    let mut outstanding: std::collections::BTreeSet<String> =
        super::storage::list_decision_index(store)
            .await?
            .into_iter()
            .filter(|(decision_id, updated)| {
                !feedback_ids.contains(decision_id)
                    && updated.is_none_or(|updated| {
                        now.signed_duration_since(updated).num_seconds() < artifact_seconds
                    })
            })
            .map(|(decision_id, _)| decision_id)
            .collect();
    outstanding.extend(
        pending_savings
            .iter()
            .map(|saving| saving.decision_id.clone()),
    );
    if outstanding.is_empty() {
        return Ok(summary);
    }
    // One pass resolves at most `OUTSTANDING_PER_TICK` of them, continuing
    // after the id the previous pass stopped at.
    //
    // The set does not drain on its own. Feedback is written only for a
    // decision whose subject job has reached `completed/` or `failed/`; a
    // decision whose job never gets there stays outstanding forever, and the
    // whole backlog was downloaded again on every tick. Measured on
    // 2026-09-03 after the planner's own read was bounded: the object API was
    // still serving 362 GETs against `state/autonomy/decisions/` in a
    // thirty-second window out of 9,580 records, and nothing else in the mix
    // came close.
    //
    // The cap rotates rather than truncating, which is the difference between
    // a bound and a blind spot: a fixed "first N" would re-read the same N
    // forever and never reach the rest, precisely because the unresolvable
    // ones sit at the front. The cursor is the last id examined, so the walk
    // is ordered, resumable and wraps to the head when it runs out -- no
    // position is dropped and no pass restarts another's walk.
    let cursor = read_outcome_cursor(store).await?;
    let ordered: Vec<String> = outstanding.into_iter().collect();
    let start = match &cursor {
        Some(last) => ordered.partition_point(|id| id <= last),
        None => usize::default(),
    };
    let window: Vec<String> = ordered
        .iter()
        .skip(start)
        .chain(ordered.iter())
        .take(OUTSTANDING_PER_TICK.min(ordered.len()))
        .cloned()
        .collect();
    let stopped_at = window.last().cloned();
    let costs = crate::scheduler::cost::collect_completed_dynamic(store).await?;
    let mut decisions = Vec::with_capacity(window.len());
    for decision_id in window {
        if let Some(decision) = super::storage::load_decision(store, &decision_id).await? {
            decisions.push(decision);
        }
    }
    if let Some(stopped_at) = stopped_at {
        write_outcome_cursor(store, &stopped_at).await?;
    }
    for decision in decisions
        .iter()
        .filter(|decision| decision.kind == DecisionKind::Placement)
    {
        let completed = store.read_job("completed", &decision.subject_id).await?;
        let failed = if completed.is_none() {
            store.read_job("failed", &decision.subject_id).await?
        } else {
            None
        };
        let Some(job) = completed.as_ref().or(failed.as_ref()) else {
            continue;
        };
        let target_id = decision
            .selected
            .as_ref()
            .and_then(|selected| selected.get("target_id"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if !feedback_ids.contains(decision.decision_id.as_str()) {
            let feedback = super::storage::PlacementFeedback {
                schema_version: SCHEMA_VERSION,
                decision_id: decision.decision_id.clone(),
                subject_id: decision.subject_id.clone(),
                target_id,
                observed_at: Utc::now().to_rfc3339(),
                startup_seconds: elapsed(Some(job.created_at.as_str()), job.started_at.as_deref()),
                runtime_seconds: elapsed(
                    job.started_at.as_deref(),
                    job.completed_at.as_deref().or(job.failed_at.as_deref()),
                ),
                realized_cost_usd: costs
                    .iter()
                    .find(|row| row.job_id == decision.subject_id)
                    .map(|row| row.cost_usd),
                succeeded: completed.is_some(),
                failure_class: failed
                    .as_ref()
                    .and_then(|job| job.error.as_deref())
                    .map(str::to_string),
            };
            match super::storage::write_feedback(store, &feedback).await {
                Ok(()) => summary.feedback_written += true as usize,
                Err(StorageError::StorageConflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let Some(cost) = costs
            .iter()
            .find(|row| row.job_id == decision.subject_id)
            .map(|row| row.cost_usd)
        else {
            continue;
        };
        for saving in pending_savings
            .iter()
            .filter(|saving| saving.decision_id == decision.decision_id)
        {
            let measurement = SavingsMeasurement {
                schema_version: SCHEMA_VERSION,
                measurement_id: format!("measurement-{}", saving.savings_id),
                savings_id: saving.savings_id.clone(),
                decision_id: saving.decision_id.clone(),
                measured_at: Utc::now().to_rfc3339(),
                realized_cost_usd: cost,
                realized_savings_usd: saving.baseline_cost_usd - cost,
                source: "completed job cost attribution".to_string(),
                source_invoice_period: None,
            };
            match super::storage::write_savings_measurement(store, &measurement).await {
                Ok(()) => summary.savings_measured += true as usize,
                Err(StorageError::StorageConflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(summary)
}

fn elapsed(start: Option<&str>, end: Option<&str>) -> Option<f64> {
    let start = DateTime::parse_from_rfc3339(start?).ok()?;
    let end = DateTime::parse_from_rfc3339(end?).ok()?;
    let milliseconds_per_second = chrono::Duration::seconds(true as i64).num_milliseconds() as f64;
    Some(end.signed_duration_since(start).num_milliseconds() as f64 / milliseconds_per_second)
}
