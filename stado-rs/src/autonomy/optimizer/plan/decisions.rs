//! What a decided job leaves behind: the savings prediction, the queued
//! job's new placement, and the blocked decision when nothing fits.
//!
//! [`persist_predicted_savings`] records what the selected target saves
//! against the next eligible one, [`update_job_placement`] rewrites the
//! queued job, and [`persist_unplaced_decision`] writes the decision for a
//! job no offer satisfies. [`placement_constraints`] and
//! [`explain_selection`] are what every decision carries as its reasoning.

use chrono::{DateTime, Utc};

use crate::autonomy::model::{DecisionKind, DecisionRecord, SavingsRecord, SCHEMA_VERSION};
use crate::autonomy::optimizer::types::PlacementCandidate;
use crate::autonomy::policy::AutonomyPolicy;
use crate::models::Job;
use crate::queue::{JobStorage, StorageError};

const TWO: f64 = (u16::BITS / u8::BITS) as f64;

pub(super) async fn persist_predicted_savings(
    store: &JobStorage,
    decision_id: &str,
    job: &Job,
    selected: &PlacementCandidate,
    candidates: &[PlacementCandidate],
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    let Some(selected_cost) = selected.expected_total_cost_usd else {
        return Ok(());
    };
    let Some(baseline) = candidates
        .iter()
        .find(|candidate| candidate.eligible && candidate.expected_total_cost_usd.is_some())
    else {
        return Ok(());
    };
    let baseline_cost = baseline.expected_total_cost_usd.unwrap_or_default();
    if baseline.target_id == selected.target_id || baseline_cost <= selected_cost {
        return Ok(());
    }
    let record = SavingsRecord {
        schema_version: SCHEMA_VERSION,
        savings_id: uuid::Uuid::new_v4().to_string(),
        decision_id: decision_id.to_string(),
        resource_id: None,
        workload: Some(job.job_id.clone()),
        provider: selected.provider,
        measurement_started_at: now.to_rfc3339(),
        measurement_ended_at: None,
        baseline_cost_usd: baseline_cost,
        predicted_cost_usd: selected_cost,
        realized_cost_usd: None,
        predicted_savings_usd: baseline_cost - selected_cost,
        realized_savings_usd: None,
        confidence: TWO.recip(),
        source_invoice_period: None,
    };
    super::storage::write_savings(store, &record).await
}

pub(super) async fn update_job_placement(
    store: &JobStorage,
    original: &Job,
    selected: &PlacementCandidate,
) -> Result<bool, StorageError> {
    let path = format!("queue/{}.json", original.job_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(false);
    };
    let current = Job::from_json(&versioned.content).map_err(|error| {
        StorageError::Other(format!("invalid queued job {}: {error}", original.job_id))
    })?;
    if current.state != crate::models::job_state::QUEUED {
        return Ok(false);
    }
    let assignment_matches = if selected.existing_capacity {
        current.assigned_to == selected.target_id
    } else {
        current.assigned_to.is_empty()
    };
    if current.provider == selected.provider.as_str()
        && current.pin_to_provider
        && assignment_matches
        && current.pinned_host.is_empty()
    {
        return Ok(false);
    }
    if !current.pinned_host.is_empty()
        && !selected
            .target_id
            .eq_ignore_ascii_case(&current.pinned_host)
    {
        return Ok(false);
    }
    // The storage rewrite recovers a transition still pending on this job
    // before it writes, and refreshes the metadata the listing reads.
    let assigned = selected
        .existing_capacity
        .then_some(selected.target_id.as_str());
    Ok(store
        .update_queued_placement(&original.job_id, selected.provider.as_str(), assigned)
        .await?
        .is_some())
}

pub(super) async fn persist_unplaced_decision(
    store: &JobStorage,
    job: &Job,
    candidates: Vec<PlacementCandidate>,
    policy: &AutonomyPolicy,
    inventory_snapshot_id: &str,
) -> Result<(), StorageError> {
    let now = Utc::now();
    let expires = now + chrono::Duration::seconds(policy.limits.decision_ttl_seconds as i64);
    let decision = DecisionRecord {
        schema_version: SCHEMA_VERSION,
        decision_id: uuid::Uuid::new_v4().to_string(),
        kind: DecisionKind::Placement,
        subject_id: job.job_id.clone(),
        created_at: now.to_rfc3339(),
        expires_at: expires.to_rfc3339(),
        inventory_snapshot_id: inventory_snapshot_id.to_string(),
        policy_version: policy.policy_version.clone(),
        selected: None,
        candidates: candidates
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?,
        constraints: placement_constraints(job, policy),
        explanation: "No candidate satisfies every placement constraint".to_string(),
        lease_token: None,
        state: "blocked".to_string(),
    };
    super::storage::write_decision(store, &decision).await
}

pub(super) fn placement_constraints(job: &Job, policy: &AutonomyPolicy) -> Vec<String> {
    let mut constraints = vec![
        format!("gpu_mem_gb >= {}", job.gpu_mem_gb),
        format!(
            "allowed_providers = {:?}",
            policy.placement.allowed_providers
        ),
    ];
    if !job.gpu_type.is_empty() {
        constraints.push(format!("gpu_type = {}", job.gpu_type));
    }
    if job.max_cost_per_hour_usd > f64::default() {
        constraints.push(format!("hourly_usd <= {:.6}", job.max_cost_per_hour_usd));
    }
    if job.pin_to_provider {
        constraints.push(format!("provider = {}", job.provider));
    }
    if !job.region.is_empty() {
        constraints.push(format!("region = {}", job.region));
    }
    constraints
}

pub(super) fn explain_selection(
    selected: &PlacementCandidate,
    candidates: &[PlacementCandidate],
) -> String {
    let rejected = candidates
        .iter()
        .filter(|candidate| !candidate.eligible)
        .count();
    format!(
        "Selected {} on {} at expected total ${:.6}; {} candidate(s) rejected by constraints",
        selected.target_id,
        selected.provider.as_str(),
        selected.expected_total_cost_usd.unwrap_or_default(),
        rejected
    )
}
