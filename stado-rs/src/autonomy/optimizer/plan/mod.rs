//! One planning pass over the queue: offers in, leased decisions out.
//!
//! [`plan_queued`] reads the offers, walks the queue in priority and deadline
//! order, scores every offer for each job, takes a lease on the cheapest
//! eligible one and writes the decision. [`decisions`] holds the records the
//! pass leaves behind. The orderings at the bottom are the two the pass
//! sorts by: which job is considered first, and which candidate wins.

mod decisions;

use std::sync::Arc;

use chrono::Utc;

use crate::autonomy::cost::PriceBook;
use crate::autonomy::model::{DecisionKind, DecisionRecord, SCHEMA_VERSION};
use crate::autonomy::policy::AutonomyPolicy;
use crate::capabilities::ProviderId;
use crate::models::Job;
use crate::providers::Provider;
use crate::queue::{JobStorage, StorageError};

// `super::storage` for the moved calls that name `super::storage::<item>`
// verbatim in `decisions`.
use crate::autonomy::storage;

use super::candidates::candidates_for_job;
use super::offers::collect_offers;
use super::types::{
    CandidateContext, CapacityOffer, CloudBudget, PlacementCandidate, PlacementRunSummary,
};

use decisions::{
    explain_selection, persist_predicted_savings, persist_unplaced_decision, placement_constraints,
    update_job_placement,
};

/// How many of the newest feedback records one planning pass reads.
///
/// The pass used to read every record ever written -- 3,642 of them on
/// 2026-09-03, one object request each, on every pass. This is the cost bound
/// that stops the archive's size from reaching the planner at all: the
/// per-target median startup time and failure ratio these records feed are
/// statistics, and a few hundred recent samples per target describe a target
/// at least as well as a month of them.
const FEEDBACK_SAMPLE_CAP: usize = 512;

#[expect(
    clippy::too_many_arguments,
    reason = "public coordinator API keeps the independent hourly and total budget controls explicit"
)]
pub async fn plan_queued(
    store: &JobStorage,
    cloud_providers: &[(String, Arc<dyn Provider>)],
    policy: &AutonomyPolicy,
    prices: &PriceBook,
    inventory_snapshot_id: &str,
    log: &dyn Fn(&str),
    mut new_cloud_hourly_budget_usd: Option<f64>,
    mut new_cloud_cost_budget_usd: Option<f64>,
) -> Result<PlacementRunSummary, StorageError> {
    let mut summary = PlacementRunSummary::default();
    if policy.mode == super::policy::AutonomyMode::Report {
        return Ok(summary);
    }
    if policy.emergency_paused {
        log("optimizer: emergency pause active; no placement mutations");
        return Ok(summary);
    }
    let (mut offers, provider_errors) = collect_offers(store, cloud_providers, policy).await?;
    summary.provider_errors = provider_errors;
    let history_rows = crate::scheduler::cost::collect_completed(store).await?;
    let wall_times = crate::scheduler::cost::wall_time_table(&history_rows);
    let feedback = super::storage::list_recent_feedback(store, FEEDBACK_SAMPLE_CAP).await?;
    let planning_now = Utc::now();
    // The planner considers the whole queue: it is placing capacity, not
    // claiming a slot, so nothing here narrows the window and nothing bounds
    // the scan.
    let mut queued = store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: usize::default(),
                scan_budget: usize::default(),
                max_gpu_mem_gb: i64::MAX,
                eligible: &|_| true,
                // Unbounded: this walk covers the whole index from the head
                // regardless, and touches no cursor.
                from_head: false,
            },
        )
        .await?;
    queued.sort_by(job_order);
    for job in queued {
        summary.considered_jobs += true as usize;
        if !job.pinned_host.is_empty() && job.pinned_host != job.assigned_to {
            continue;
        }
        let candidate_context = CandidateContext {
            policy,
            feedback: &feedback,
            budget: CloudBudget {
                hourly_usd: new_cloud_hourly_budget_usd,
                total_usd: new_cloud_cost_budget_usd,
            },
            now: planning_now,
        };
        let candidates = candidates_for_job(&job, &offers, prices, &wall_times, &candidate_context);
        let selected = candidates
            .iter()
            .filter(|candidate| candidate.eligible)
            .min_by(|left, right| candidate_order(left, right));
        let Some(selected) = selected else {
            summary.no_eligible_target += true as usize;
            persist_unplaced_decision(store, &job, candidates, policy, inventory_snapshot_id)
                .await?;
            continue;
        };
        let decision_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let lease = super::storage::acquire_placement_lease(
            store,
            &job.job_id,
            &decision_id,
            "coordinator-placement",
            policy.limits.decision_ttl_seconds,
            now,
        )
        .await?;
        let Some(lease) = lease else {
            summary.active_lease_skips += true as usize;
            continue;
        };
        let explanation = explain_selection(selected, &candidates);
        let decision = DecisionRecord {
            schema_version: SCHEMA_VERSION,
            decision_id: decision_id.clone(),
            kind: DecisionKind::Placement,
            subject_id: job.job_id.clone(),
            created_at: now.to_rfc3339(),
            expires_at: lease.expires_at.clone(),
            inventory_snapshot_id: inventory_snapshot_id.to_string(),
            policy_version: policy.policy_version.clone(),
            selected: Some(serde_json::to_value(selected)?),
            candidates: candidates
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?,
            constraints: placement_constraints(&job, policy),
            explanation,
            lease_token: Some(lease.token.clone()),
            state: "leased".to_string(),
        };
        if let Err(error) = super::storage::write_decision(store, &decision).await {
            let _ = super::storage::release_placement_lease(store, &job.job_id, &lease.token).await;
            return Err(error);
        }
        summary.decided_jobs += true as usize;
        match update_job_placement(store, &job, selected).await {
            Ok(true) => {
                summary.changed_jobs += true as usize;
                if let Err(error) =
                    persist_predicted_savings(store, &decision_id, &job, selected, &candidates, now)
                        .await
                {
                    log(&format!(
                        "optimizer: savings record for {} failed: {error}",
                        job.job_id
                    ));
                }
                reserve_offer(&mut offers, selected);
                if !selected.existing_capacity
                    && matches!(
                        selected.provider,
                        ProviderId::Gcp | ProviderId::Aws | ProviderId::Azure
                    )
                {
                    if let (Some(remaining), Some(hourly)) = (
                        new_cloud_hourly_budget_usd.as_mut(),
                        selected.hourly_compute_usd,
                    ) {
                        *remaining = (*remaining - hourly).max(f64::default());
                    }
                    if let (Some(remaining), Some(cost)) = (
                        new_cloud_cost_budget_usd.as_mut(),
                        selected.expected_total_cost_usd,
                    ) {
                        *remaining = (*remaining - cost).max(f64::default());
                    }
                }
            }
            Ok(false) => {}
            Err(error) => {
                let _ =
                    super::storage::release_placement_lease(store, &job.job_id, &lease.token).await;
                return Err(error);
            }
        }
    }
    Ok(summary)
}

fn reserve_offer(offers: &mut [CapacityOffer], selected: &PlacementCandidate) {
    if let Some(offer) = offers.iter_mut().find(|offer| {
        offer.target_id == selected.target_id && offer.accelerator_type == selected.accelerator_type
    }) {
        offer.available_instances = offer.available_instances.saturating_sub(1);
    }
}

fn job_order(left: &Job, right: &Job) -> std::cmp::Ordering {
    right.priority.cmp(&left.priority).then_with(|| {
        match (job_deadline(left), job_deadline(right)) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    })
}

fn job_deadline(job: &Job) -> Option<chrono::DateTime<Utc>> {
    job.deadline_at
        .as_deref()
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|deadline| deadline.with_timezone(&Utc))
}

fn candidate_order(left: &PlacementCandidate, right: &PlacementCandidate) -> std::cmp::Ordering {
    left.expected_total_cost_usd
        .unwrap_or(f64::INFINITY)
        .partial_cmp(&right.expected_total_cost_usd.unwrap_or(f64::INFINITY))
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            left.expected_finish_seconds
                .partial_cmp(&right.expected_finish_seconds)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| left.target_id.cmp(&right.target_id))
}
