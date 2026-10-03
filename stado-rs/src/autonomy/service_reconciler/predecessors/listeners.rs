//! Listener handoffs of role units, each change made under the unit's lease.
//!
//! A handoff reads the host's record, decides, and writes it again; two
//! reconcilers doing that at once could overwrite the autostart scopes the
//! record saved. So every step that changes something goes through the pass's
//! one mutation gate for the exact old unit: the action limit, the live pause
//! and circuit breaker, and the unit's placement lease, held from the record
//! read through the write. Standing that needs no change is only read.

use std::collections::BTreeSet;

use crate::deploy::service::{self, Handoff, ManagedService};
use crate::deploy::service_catalog::RoleUnit;
use crate::deploy::Runner;
use crate::queue::StorageError;
use crate::targets::ComputeTarget;

use super::super::gate::MutationGate;
use super::super::receipts::{ServiceReconcileOutcome, ServiceReconcileSummary};

/// One replacement on its host, as the pass found it, with the units there
/// whose work is a listener role of it.
pub(super) struct Replaced<'a> {
    pub(super) target: &'a ComputeTarget,
    pub(super) running: &'a ManagedService,
    pub(super) roles: &'a [RoleUnit],
    pub(super) active: bool,
}

/// Take the next step of every listener handoff of `replaced`'s role units.
pub(super) async fn hand_over_listeners(
    replaced: &Replaced<'_>,
    declared: &BTreeSet<(String, String)>,
    runner: &Runner,
    gate: &mut MutationGate<'_>,
    summary: &mut ServiceReconcileSummary,
) -> Result<Vec<ServiceReconcileOutcome>, StorageError> {
    let host = &replaced.running.host;
    let stopped = !replaced.active;
    let row =
        |unit: &str, classification: &str, changed: bool, detail: String| ServiceReconcileOutcome {
            host: host.clone(),
            service: unit.to_string(),
            unit: unit.to_string(),
            beacon_state: "not-used".to_string(),
            endpoint_state: "not-used".to_string(),
            classification: classification.to_string(),
            action: "hand_over_listener".to_string(),
            changed,
            detail: format!("replaced by {}: {detail}", replaced.running.name),
        };
    let mut outcomes = Vec::new();
    for role in replaced
        .roles
        .iter()
        .filter(|role| service::listener_role(role))
    {
        let may_start = replaced.active && declared.contains(&(host.clone(), role.unit.clone()));
        let standing =
            service::listener_standing(replaced.target, replaced.running, role, stopped, runner)
                .await;
        let due = match standing {
            Ok(Handoff::Complete | Handoff::Restore { .. }) => true,
            Ok(Handoff::Start) => may_start,
            Ok(Handoff::Waiting(detail)) => {
                outcomes.push(row(&role.unit, "awaiting_resolver", false, detail));
                false
            }
            Ok(Handoff::Kept(detail)) if replaced.active => {
                outcomes.push(row(&role.unit, "kept", false, detail));
                false
            }
            Ok(Handoff::Kept(_) | Handoff::Retained(_)) => false,
            Err(error) => {
                summary.failures += 1;
                outcomes.push(row(&role.unit, "repair_failed", false, error.to_string()));
                false
            }
        };
        if !due {
            continue;
        }
        let (subject, lease) = match gate.admit(host, &role.unit).await? {
            Ok(admitted) => admitted,
            Err(refusal) => {
                summary.blocked += 1;
                outcomes.push(row(
                    &role.unit,
                    refusal.classification,
                    false,
                    refusal.detail,
                ));
                continue;
            }
        };
        let step = service::hand_over_role(
            replaced.target,
            replaced.running,
            role,
            may_start,
            stopped,
            runner,
        )
        .await;
        let result = if step.state == "failed" {
            Err(step.detail.clone())
        } else {
            Ok(step)
        };
        match gate.release(&subject, &lease, result).await {
            Ok(step) => {
                let changed = matches!(step.state.as_str(), "retired" | "handed_over" | "restored");
                if changed {
                    summary.changed += 1;
                    gate.record(None).await?;
                }
                outcomes.push(row(&step.unit, &step.state, changed, step.detail));
            }
            Err(error) => {
                summary.failures += 1;
                gate.record(Some(&error)).await?;
                outcomes.push(row(&role.unit, "repair_failed", false, error));
            }
        }
    }
    Ok(outcomes)
}
