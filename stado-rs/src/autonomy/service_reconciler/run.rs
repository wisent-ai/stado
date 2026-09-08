//! One pass: a row per declared service, one shared mutation gate for every
//! repair, and the receipts the pass leaves behind.

use chrono::{SecondsFormat, Utc};

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service;
use crate::queue::{JobStorage, StorageError};

use super::endpoint::{endpoint_states, EndpointState};
use super::receipts::{
    alert_transitions, persist_report, ServiceReconcileOutcome, ServiceReconcileReport,
    ServiceReconcileSummary,
};
use super::repair::{
    reconcile_beacon, reconcile_observed, reconcile_undeclared, reconcile_unreachable,
};
use super::{LATEST_REPORT, SCHEMA_VERSION};

/// The two classification words the pass records from an `else` branch. The
/// recorded strings are unchanged; the write gate requires that a word an
/// `else` branch stores be named once outside it.
const UNKNOWN_EVIDENCE: &str = "unknown";
const LEASE_BLOCKED: &str = "lease_blocked";

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
    let mut mutations = usize::default();

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
        if mutations >= policy.limits.max_actions_per_tick {
            outcome.classification = "action_limit".to_string();
            outcome.detail = "service action limit reached for this autonomy tick".to_string();
            summary.blocked += 1;
            outcomes.push(outcome);
            continue;
        }
        let control = crate::autonomy::storage::load_control(store).await?;
        if control.emergency_paused || control.circuit_open_at(Utc::now()) {
            outcome.classification = "control_blocked".to_string();
            outcome.detail = "autonomy pause or circuit breaker became active".to_string();
            summary.blocked += 1;
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
        let lease_subject = format!(
            "service:{}:{}",
            status.service.host,
            status.service.unit_id()
        );
        let Some(lease) = crate::autonomy::storage::acquire_placement_lease(
            store,
            &lease_subject,
            &decision_id,
            "service-reconciler",
            policy.limits.decision_ttl_seconds,
            Utc::now(),
        )
        .await?
        else {
            outcome.classification = LEASE_BLOCKED.to_string();
            outcome.detail = "another reconciler owns this service mutation".to_string();
            summary.blocked += 1;
            outcomes.push(outcome);
            continue;
        };
        mutations += 1;
        let mut result = match planned_action {
            "beacon_repair" => reconcile_beacon(&status, &target, &runner).await,
            "adopt" => reconcile_observed(&status, &target, &runner).await,
            "ensure" => reconcile_unreachable(&status, &target, &runner).await,
            "host_probe" => reconcile_undeclared(&status, &target, &runner).await,
            _ => unreachable!(),
        };
        match crate::autonomy::storage::release_placement_lease(store, &lease_subject, &lease.token)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                result = Err(
                    "service action finished, but mutation lease ownership changed before release"
                        .to_string(),
                );
            }
            Err(error) => {
                result = Err(format!(
                    "service action finished, but mutation lease release failed: {error}"
                ));
            }
        }
        match result {
            Ok((action, changed, detail)) => {
                outcome.classification = "reconciled".to_string();
                outcome.action = action;
                outcome.changed = changed;
                outcome.detail = detail;
                if changed {
                    summary.changed += 1;
                }
                crate::autonomy::storage::record_mutation_outcome(
                    store,
                    true,
                    None,
                    policy.limits.circuit_breaker_failures,
                    policy.limits.circuit_breaker_cooldown_seconds,
                )
                .await?;
            }
            Err(error) => {
                outcome.classification = if error.starts_with("endpoint responds")
                    || error.contains("ownership is not proven")
                {
                    "identity_unresolved".to_string()
                } else if error.contains("nothing declares") {
                    "declaration_incomplete".to_string()
                } else {
                    "repair_failed".to_string()
                };
                outcome.detail = error.clone();
                summary.failures += 1;
                // Only a mutation that actually failed on a host feeds the
                // circuit breaker. `identity_unresolved` and
                // `declaration_incomplete` are refusals computed before any
                // host command ran; counting them opened the breaker on four
                // incomplete declarations and starved every healthy repair
                // behind them, fifteen minutes per tick, forever.
                if outcome.classification == "repair_failed" {
                    crate::autonomy::storage::record_mutation_outcome(
                        store,
                        false,
                        Some(&error),
                        policy.limits.circuit_breaker_failures,
                        policy.limits.circuit_breaker_cooldown_seconds,
                    )
                    .await?;
                }
            }
        }
        outcomes.push(outcome);
    }

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
