//! One pass: a row per declared service, one shared mutation gate for every
//! repair, and the receipts the pass leaves behind.

use chrono::{SecondsFormat, Utc};

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service;
use crate::queue::{JobStorage, StorageError};

use super::endpoint::{endpoint_states, EndpointState};
use super::gate::MutationGate;
use super::receipts::{
    alert_transitions, persist_report, ServiceReconcileOutcome, ServiceReconcileReport,
    ServiceReconcileSummary,
};
use super::repair::{
    reconcile_beacon, reconcile_observed, reconcile_undeclared, reconcile_unreachable, FailureKind,
    RepairRefused,
};
use super::{LATEST_REPORT, SCHEMA_VERSION};

/// The classification word the pass records from an `else` branch. The
/// recorded string is unchanged; the write gate requires that a word an
/// `else` branch stores be named once outside it.
const UNKNOWN_EVIDENCE: &str = "unknown";

pub async fn reconcile(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    log: &dyn Fn(&str),
) -> Result<ServiceReconcileReport, StorageError> {
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
    let decision_id = format!("service-reconcile-{}", created_at.replace(':', "-"));
    let previous =
        crate::autonomy::storage::read_json::<ServiceReconcileReport>(store, LATEST_REPORT).await?;
    let statuses = service::list_services(store)
        .await
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let sweep = crate::cli::service_verify::sweep(None).await;
    let (findings, sweep_error) = match sweep {
        Ok(findings) => (findings, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let endpoints = endpoint_states(&findings);
    let runner = crate::deploy::production_runner();
    let mut summary = ServiceReconcileSummary {
        services: statuses.len(),
        ..ServiceReconcileSummary::default()
    };
    let mut outcomes = Vec::new();
    let mut gate = MutationGate::new(store, policy, &decision_id);
    let replacements = super::predecessors::replacements(&statuses);
    let declared = super::predecessors::declared_units(&statuses);
    // A host that declares a product's old units but not the product's one
    // process gets that process now; the next pass retires the old units.
    outcomes.extend(
        super::replacements::ensure_replacements(&statuses, policy, &mut gate, &mut summary)
            .await?,
    );

    for status in statuses {
        let is_beacon = status.service.unit_id().contains("host-health-beacon")
            || status.service.name.contains("host-health-beacon");
        let mut outcome = ServiceReconcileOutcome {
            host: status.service.host.clone(),
            service: status.service.name.clone(),
            unit: status.service.unit_id().to_string(),
            beacon_state: status.state.clone(),
            endpoint_state: "not-used".to_string(),
            classification: String::new(),
            action: "none".to_string(),
            changed: false,
            detail: String::new(),
        };

        // Which repair this row needs. `None` means the row is recorded and
        // left alone; every `Some` goes through one shared mutation gate below
        // so no repair path can grow its own weaker safety checks.
        let kind: Option<&'static str> = if status.state == service::STATE_UNKNOWN {
            if is_beacon {
                // The one exception to "unknown evidence mutates nothing":
                // the beacon's own death is what makes everything unknown,
                // and the host channel is its evidence and its repair path.
                Some("beacon_repair")
            } else {
                summary.unknown += 1;
                outcome.classification = UNKNOWN_EVIDENCE.to_string();
                outcome.detail = status.detail.clone();
                outcomes.push(outcome);
                continue;
            }
        } else if status.state != service::STATE_MISSING && status.state != service::STATE_FAILED {
            continue;
        } else {
            // A retired unit's absence is the intended state: the product's
            // one process unloaded it and removed its launch agent. Repairing
            // it would start it again beside that process.
            let retired = crate::deploy::service_catalog::retired_by(status.service.unit_id())
                .ok()
                .flatten()
                .or_else(|| {
                    crate::deploy::service_catalog::retired_by(&status.service.name)
                        .ok()
                        .flatten()
                });
            if let Some(replacement) = retired {
                outcome.classification = "retired".to_string();
                outcome.detail = crate::deploy::service_catalog::retired_sentence(
                    status.service.unit_id(),
                    &replacement,
                );
                outcomes.push(outcome);
                continue;
            }
            // A role unit whose role this host's product process runs is
            // retired there, although its declaration remains: the same
            // question retirement asks decides it, so it is never reasserted.
            if let Some(detail) =
                super::predecessors::taken_over(&status.service, &replacements, &runner).await
            {
                outcome.classification = "retired".to_string();
                outcome.detail = detail;
                outcomes.push(outcome);
                continue;
            }
            // A `failed` unit is the same repair as a missing one: the unit
            // exists, nothing runs under it, and `ensure` restarts in place.
            summary.missing += 1;
            let endpoint = endpoints
                .get(&status.service.name)
                .copied()
                .unwrap_or(EndpointState::Absent);
            outcome.endpoint_state = endpoint.word().to_string();
            if status.service.source != service::SOURCE_REGISTRY {
                outcome.classification = "externally_managed".to_string();
                outcome.detail = "service belongs to the fixed recovery program".to_string();
                summary.blocked += 1;
                outcomes.push(outcome);
                continue;
            }
            if let Some(error) = &sweep_error {
                outcome.classification = "endpoint_unverified".to_string();
                outcome.endpoint_state = EndpointState::Unverified.word().to_string();
                outcome.detail = format!("reachability sweep did not complete: {error}");
                summary.blocked += 1;
                outcomes.push(outcome);
                continue;
            }
            match endpoint {
                EndpointState::Observed => Some("adopt"),
                EndpointState::Unreachable => Some("ensure"),
                // Not in the service directory at all: no endpoint exists to
                // disprove, so the host channel is the evidence instead.
                EndpointState::Absent => Some("host_probe"),
                EndpointState::Unverified => {
                    outcome.classification = "endpoint_unverified".to_string();
                    outcome.detail =
                        "unit is missing, but endpoint absence was not proven".to_string();
                    summary.blocked += 1;
                    outcomes.push(outcome);
                    continue;
                }
            }
        };
        let Some(planned_action) = kind else { continue };

        summary.planned += 1;
        outcome.action = format!("planned_{planned_action}");
        if policy.mode == AutonomyMode::Report || policy.emergency_paused {
            outcome.classification = "planned".to_string();
            outcome.detail = if policy.emergency_paused {
                "mutation blocked by autonomy emergency pause".to_string()
            } else {
                format!("report mode: {planned_action} was planned but not executed")
            };
            outcomes.push(outcome);
            continue;
        }
        let target = match crate::deploy::host_channel::canonical_target(&status.service.host).await
        {
            Ok(target) => target,
            Err(error) => {
                outcome.classification = "repair_failed".to_string();
                outcome.detail = error.to_string();
                summary.failures += 1;
                outcomes.push(outcome);
                continue;
            }
        };
        let (subject, lease) = match gate
            .admit(&status.service.host, status.service.unit_id())
            .await?
        {
            Ok(admitted) => admitted,
            Err(refusal) => {
                outcome.classification = refusal.classification.to_string();
                outcome.detail = refusal.detail;
                summary.blocked += 1;
                outcomes.push(outcome);
                continue;
            }
        };
        // The takeover may have been recorded since the row was judged: asked
        // again under the lease, and once more after the repair, which retires
        // a unit the repair brought back after a takeover retired it.
        let taken = if planned_action == "beacon_repair" {
            None
        } else {
            super::predecessors::taken_over(&status.service, &replacements, &runner).await
        };
        let repaired = taken.is_none();
        let result = match (taken, planned_action) {
            (Some(detail), _) => Ok(("retired".to_string(), false, detail)),
            (None, "beacon_repair") => reconcile_beacon(&status, &target, &runner).await,
            (None, "adopt") => reconcile_observed(&status, &target, &runner).await,
            (None, "ensure") => reconcile_unreachable(&status, &target, &runner).await,
            (None, "host_probe") => reconcile_undeclared(&status, &target, &runner).await,
            _ => unreachable!(),
        };
        // A failed repair may still have loaded the unit (autostart is enabled
        // before startup is judged), so the retirement is retried either way;
        // a failed repair stays failed, and a failed retirement fails the row.
        let retaken = if repaired {
            super::predecessors::retake(&status.service, &target, &runner).await
        } else {
            None
        };
        let result = match (result, retaken) {
            (result, None) => result,
            (Ok((_, _, detail)), Some(Ok(undone))) => {
                Ok(("retired".to_string(), true, format!("{detail}; {undone}")))
            }
            (Ok((_, _, detail)), Some(Err(failed))) => {
                Err(RepairRefused::from(format!("{detail}; {failed}")))
            }
            (Err(error), Some(Ok(undone) | Err(undone))) => Err(RepairRefused::new(
                error.kind,
                format!("{}; {undone}", error.detail),
            )),
        };
        let result = gate.release(&subject, &lease, result).await;
        match result {
            Ok((action, changed, detail)) => {
                outcome.classification = "reconciled".to_string();
                outcome.action = action;
                outcome.changed = changed;
                outcome.detail = detail;
                if changed {
                    summary.changed += 1;
                }
                gate.record(None).await?;
            }
            Err(error) => {
                outcome.classification = error.kind.classification().to_string();
                outcome.detail = error.detail.clone();
                summary.failures += 1;
                // Only a mutation that actually failed on a host feeds the
                // circuit breaker. `identity_unresolved` and
                // `declaration_incomplete` are refusals computed before any
                // host command ran; counting them opened the breaker on four
                // incomplete declarations and starved every healthy repair
                // behind them, fifteen minutes per tick, forever.
                if error.kind == FailureKind::RepairFailed {
                    gate.record(Some(&error.detail)).await?;
                }
            }
        }
        outcomes.push(outcome);
    }
    outcomes.extend(
        super::predecessors::retire(
            &replacements,
            &declared,
            &findings,
            policy,
            &runner,
            &mut gate,
            &mut summary,
        )
        .await?,
    );

    let report = ServiceReconcileReport {
        schema_version: SCHEMA_VERSION,
        created_at,
        mode: policy.mode,
        summary,
        outcomes,
    };
    persist_report(store, &report).await?;
    alert_transitions(previous.as_ref(), &report).await;
    log(&format!(
        "service reconciliation: services={} missing={} unknown={} planned={} changed={} blocked={} failures={}",
        report.summary.services,
        report.summary.missing,
        report.summary.unknown,
        report.summary.planned,
        report.summary.changed,
        report.summary.blocked,
        report.summary.failures,
    ));
    Ok(report)
}
