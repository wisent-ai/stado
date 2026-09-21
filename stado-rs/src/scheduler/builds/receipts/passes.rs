//! Turning a finished build into a verdict on the deliveries it carried.
//!
//! A qualification pass names the build jobs it is waiting on. When those
//! jobs reach a terminal state, the pass settles and every delivery it
//! answers for settles with it: `verified` when the build and the product's
//! own tests passed, `failed` with the job's own sentence when they did not.
//!
//! This runs inside the same fenced write that records the runs, so a pass
//! cannot observe a half-written outcome, and a delivery cannot be verified
//! by a run that was never recorded.

use serde_json::Value;

use crate::models::isoformat_utc;
use crate::targets::{Delivery, DeliveryState, QualificationPass, DELIVERIES_KEY, PASSES_KEY};

use super::RunOutcome;

/// Settle every open pass whose jobs are all accounted for by `outcomes` or
/// by runs already recorded in `document`.
///
/// Returns how many passes settled, for the one log line that says so.
pub(super) fn settle_passes(document: &mut Value, outcomes: &[RunOutcome]) -> usize {
    let verdicts: Vec<(String, bool, Option<String>)> = outcomes
        .iter()
        .map(|outcome| {
            (
                outcome.run.job_id.clone(),
                outcome.run.status == "succeeded",
                outcome.run.reason.clone(),
            )
        })
        .collect();
    if verdicts.is_empty() {
        return 0;
    }
    let mut settled: Vec<(Vec<String>, bool, Option<String>)> = Vec::new();
    let Some(entries) = document.get_mut(PASSES_KEY).and_then(Value::as_array_mut) else {
        return 0;
    };
    for entry in entries.iter_mut() {
        let Ok(mut pass) = serde_json::from_value::<QualificationPass>(entry.clone()) else {
            continue;
        };
        if !pass.open() {
            continue;
        }
        // A pass is answered only when every job it named has a verdict: a
        // green darwin build says nothing about the linux one it was
        // submitted beside.
        let mut answers: Vec<(bool, Option<String>)> = Vec::new();
        for job_id in pass.jobs.values() {
            let Some((_, succeeded, reason)) = verdicts.iter().find(|(id, _, _)| id == job_id)
            else {
                answers.clear();
                break;
            };
            answers.push((*succeeded, reason.clone()));
        }
        if answers.is_empty() {
            continue;
        }
        let failed = answers.iter().find(|(succeeded, _)| !succeeded);
        pass.settled_at = Some(isoformat_utc(chrono::Utc::now()));
        match failed {
            Some((_, reason)) => {
                pass.status = QualificationPass::FAILED.to_string();
                pass.reason = Some(reason.clone().unwrap_or_else(|| {
                    "the build or its tests failed; read the job log with `stado job watch`"
                        .to_string()
                }));
            }
            None => pass.status = QualificationPass::PASSED.to_string(),
        }
        settled.push((
            pass.deliveries.clone(),
            pass.status == QualificationPass::PASSED,
            pass.reason.clone(),
        ));
        if let Ok(value) = serde_json::to_value(&pass) {
            *entry = value;
        }
    }
    if settled.is_empty() {
        return 0;
    }
    for (deliveries, passed, reason) in &settled {
        settle_deliveries(document, deliveries, *passed, reason.as_deref());
    }
    settled.len()
}

/// Write one pass's verdict onto the deliveries it answered for.
fn settle_deliveries(
    document: &mut Value,
    deliveries: &[String],
    passed: bool,
    reason: Option<&str>,
) {
    let Some(entries) = document
        .get_mut(DELIVERIES_KEY)
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for entry in entries.iter_mut() {
        let Ok(mut delivery) = serde_json::from_value::<Delivery>(entry.clone()) else {
            continue;
        };
        if !deliveries.contains(&delivery.id) || delivery.state != DeliveryState::Qualifying {
            continue;
        }
        delivery.state = if passed {
            DeliveryState::Verified
        } else {
            DeliveryState::Failed
        };
        delivery.settled_at = Some(isoformat_utc(chrono::Utc::now()));
        delivery.reason = reason.map(str::to_string);
        if let Ok(value) = serde_json::to_value(&delivery) {
            *entry = value;
        }
    }
}
