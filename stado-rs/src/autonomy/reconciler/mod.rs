//! Continuous resource reconciliation using the existing immutable resource-plan engine.
//!
//! The components are the seams this file already carried: [`mutations`] runs
//! an approved plan under the mutation-slot lease and the circuit breaker,
//! [`schedules`] reconciles the start and stop expressions a resource rule
//! declares, [`idle`] classifies idle and orphaned resources and builds the
//! drift plan from them, and [`resource`] renders the locator, age and
//! ownership fields both plan builders write. Every name a caller outside this
//! module uses is re-exported here, so `crate::autonomy::reconciler::<item>`
//! resolves exactly as before.

mod idle;
mod mutations;
mod resource;
mod schedules;

// `policy` and `storage` are rebound here for the moved lines that name
// `super::policy::<item>` verbatim in `schedules` and `idle`, and
// `super::storage::<item>` verbatim in `mutations` and `schedules`.
use crate::autonomy::policy;
use crate::autonomy::storage;

use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

use super::model::InventorySnapshot;
use super::policy::{AutonomyMode, AutonomyPolicy};

pub use idle::build_plan;

use mutations::execute_with_circuit;
use schedules::reconcile_schedules;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReconcileSummary {
    pub operation_id: Option<String>,
    pub findings: usize,
    pub automatic_actions: usize,
    pub scheduled_actions: usize,
    pub executed: bool,
    pub blocked_reason: Option<String>,
}

pub async fn reconcile(
    store: &JobStorage,
    snapshot: &InventorySnapshot,
    policy: &AutonomyPolicy,
    configuration_fingerprint: &str,
    log: &dyn Fn(&str),
) -> Result<ReconcileSummary, StorageError> {
    let mut summary = ReconcileSummary::default();
    if policy.emergency_paused {
        summary.blocked_reason = Some("emergency pause is active".to_string());
        return Ok(summary);
    }
    if !snapshot.complete {
        summary.blocked_reason = Some("inventory is incomplete".to_string());
        return Ok(summary);
    }
    if policy.mode != AutonomyMode::Report {
        summary.scheduled_actions =
            reconcile_schedules(store, snapshot, policy, configuration_fingerprint).await?;
        if summary.scheduled_actions > usize::default() {
            summary.automatic_actions = summary.scheduled_actions;
            summary.executed = true;
            return Ok(summary);
        }
    }
    let plan = build_plan(snapshot, policy, configuration_fingerprint)?;
    summary.operation_id = Some(plan.operation_id.clone());
    summary.findings = plan.findings.len();
    summary.automatic_actions = plan.actions.len();
    super::storage::write_json(
        store,
        &format!("state/autonomy/plans/{}.json", plan.operation_id),
        &plan,
        false,
    )
    .await?;
    if policy.mode == AutonomyMode::Report || plan.actions.is_empty() {
        return Ok(summary);
    }
    log(&format!(
        "autonomy: executing {} bounded owned-resource action(s) from plan {}",
        plan.actions.len(),
        plan.operation_id
    ));
    execute_with_circuit(store, &plan, policy).await?;
    summary.executed = true;
    Ok(summary)
}
