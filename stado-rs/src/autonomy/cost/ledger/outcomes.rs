//! The outcome pass: feedback and savings measurements for decisions whose
//! subject job has finished.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autonomy::model::{DecisionKind, SavingsMeasurement};
use crate::autonomy::policy::AutonomyPolicy;
use crate::queue::{JobStorage, StorageError};

/// What one outcome pass wrote: the decisions given feedback and the savings
/// records measured, and the placement decisions it could not give feedback
/// because their selection names no target.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutcomeSummary {
    pub feedback_written: Vec<String>,
    pub savings_measured: Vec<String>,
    #[serde(default)]
    pub feedback_unattributed: Vec<String>,
}

impl OutcomeSummary {
    /// Whether the pass wrote or withheld anything.
    pub fn is_empty(&self) -> bool {
        self.feedback_written.is_empty()
            && self.savings_measured.is_empty()
            && self.feedback_unattributed.is_empty()
    }
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
/// A measurement that downloads every decision, feedback, savings and
/// measurement record ever written and then reads one or two job objects per
/// decision costs tens of thousands of object reads per tick against a store
/// that also serves the public release channel, and starves a release
/// download of bandwidth.
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
    let retention_days = policy.idle.artifact_days;
    let mut outstanding: std::collections::BTreeSet<String> =
        super::storage::list_decision_index(store)
            .await?
            .into_iter()
            .filter(|(decision_id, updated)| {
                !feedback_ids.contains(decision_id)
                    && updated.is_none_or(|updated| within_retention(now, updated, retention_days))
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
    let costs = crate::scheduler::cost::collect_completed(store).await?;
    let mut decisions = Vec::with_capacity(outstanding.len());
    for decision_id in outstanding {
        if let Some(decision) = super::storage::load_decision(store, &decision_id).await? {
            decisions.push(decision);
        }
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
        // Feedback is what placement learns a target from. A decision whose
        // selection names no target is withheld from feedback and named in
        // the summary, since no target earned or lost anything by it.
        let target_id = decision
            .selected
            .as_ref()
            .and_then(|selected| selected.get("target_id"))
            .and_then(Value::as_str);
        match (
            feedback_ids.contains(decision.decision_id.as_str()),
            target_id,
        ) {
            (true, _) => {}
            (false, None) => summary
                .feedback_unattributed
                .push(decision.decision_id.clone()),
            (false, Some(target_id)) => {
                let target_id = target_id.to_string();
                let feedback = super::storage::PlacementFeedback {
                    decision_id: decision.decision_id.clone(),
                    subject_id: decision.subject_id.clone(),
                    target_id,
                    observed_at: Utc::now().to_rfc3339(),
                    startup_seconds: elapsed(
                        &decision.subject_id,
                        Some(job.created_at.as_str()),
                        job.started_at.as_deref(),
                    )?,
                    runtime_seconds: elapsed(
                        &decision.subject_id,
                        job.started_at.as_deref(),
                        job.completed_at.as_deref().or(job.failed_at.as_deref()),
                    )?,
                    realized_cost_usd: costs
                        .iter()
                        .find(|row| row.job_id == decision.subject_id)
                        .and_then(|row| row.cost_usd),
                    succeeded: completed.is_some(),
                    failure_class: failed
                        .as_ref()
                        .and_then(|job| job.error.as_deref())
                        .map(str::to_string),
                };
                match super::storage::write_feedback(store, &feedback).await {
                    Ok(()) => summary.feedback_written.push(feedback.decision_id.clone()),
                    Err(StorageError::StorageConflict(_)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let Some(cost) = costs
            .iter()
            .find(|row| row.job_id == decision.subject_id)
            .and_then(|row| row.cost_usd)
        else {
            continue;
        };
        for saving in pending_savings
            .iter()
            .filter(|saving| saving.decision_id == decision.decision_id)
        {
            let measurement = SavingsMeasurement {
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
                Ok(()) => summary
                    .savings_measured
                    .push(measurement.savings_id.clone()),
                Err(StorageError::StorageConflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(summary)
}

/// Whether a decision last updated at `updated` is still inside an artifact
/// retention of `days` at `now`; one dated after `now` is as young as one
/// dated now.
fn within_retention(now: DateTime<Utc>, updated: DateTime<Utc>, days: u64) -> bool {
    let age = now.signed_duration_since(updated).num_days();
    age.is_negative() || age.unsigned_abs() < days
}

/// Seconds between two of job `job`'s stamps: none while either is not
/// recorded yet, and a refusal naming the job and the stamp when one is
/// recorded but is not an RFC 3339 time.
fn elapsed(job: &str, start: Option<&str>, end: Option<&str>) -> Result<Option<f64>, StorageError> {
    let (Some(start), Some(end)) = (start, end) else {
        return Ok(None);
    };
    let read = |stamp: &str| {
        DateTime::parse_from_rfc3339(stamp).map_err(|error| {
            StorageError::Other(format!(
                "job {job} records the time {stamp:?}, which is not an RFC 3339 time: {error}"
            ))
        })
    };
    let (start, end) = (read(start)?, read(end)?);
    Ok(Some(end.signed_duration_since(start).as_seconds_f64()))
}
