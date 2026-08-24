//! Reconcile registry-declared services against fresh host and endpoint facts.
//!
//! A beacon is the unit-side fact and a reachability sweep is the endpoint-side
//! fact. Neither is allowed to stand in for the other. Stale or unverified
//! evidence causes no mutation; a freshly missing unit with a freshly
//! unreachable endpoint may be recreated idempotently; a responding endpoint
//! is never duplicated merely because the beacon omitted its unit.

use std::collections::BTreeMap;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::deploy::service::{self, ManagedService, ServiceStatus};
use crate::monitor::alerts;
use crate::observations::{OBSERVED, UNREACHABLE};
use crate::queue::{JobStorage, StorageError};

const LATEST_REPORT: &str = "autonomy/services/latest.json";
const REPORT_PREFIX: &str = "autonomy/services/runs";
const SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceReconcileSummary {
    pub services: usize,
    pub missing: usize,
    pub unknown: usize,
    pub planned: usize,
    pub changed: usize,
    pub blocked: usize,
    pub failures: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceReconcileOutcome {
    pub host: String,
    pub service: String,
    pub unit: String,
    pub beacon_state: String,
    pub endpoint_state: String,
    pub classification: String,
    pub action: String,
    pub changed: bool,
    pub detail: String,
}

impl ServiceReconcileOutcome {
    fn key(&self) -> String {
        format!("{}:{}", self.host, self.service)
    }

    fn needs_alert(&self) -> bool {
        matches!(
            self.classification.as_str(),
            "repair_failed"
                | "identity_unresolved"
                | "declaration_incomplete"
                | "endpoint_unverified"
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceReconcileReport {
    pub schema_version: u16,
    pub created_at: String,
    pub mode: AutonomyMode,
    pub summary: ServiceReconcileSummary,
    pub outcomes: Vec<ServiceReconcileOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointState {
    Observed,
    Unreachable,
    Unverified,
    Absent,
}

impl EndpointState {
    fn word(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Unreachable => "unreachable",
            Self::Unverified => "unverified",
            Self::Absent => "absent",
        }
    }
}

fn endpoint_states(
    findings: &[crate::cli::service_verify::Finding],
) -> BTreeMap<String, EndpointState> {
    let mut states = BTreeMap::new();
    for finding in findings.iter().filter(|finding| finding.probed) {
        let next = match finding.state {
            OBSERVED => EndpointState::Observed,
            UNREACHABLE => EndpointState::Unreachable,
            _ => EndpointState::Unverified,
        };
        states
            .entry(finding.service.clone())
            .and_modify(|current| {
                *current = match (*current, next) {
                    (EndpointState::Observed, _) | (_, EndpointState::Observed) => {
                        EndpointState::Observed
                    }
                    (EndpointState::Unverified, _) | (_, EndpointState::Unverified) => {
                        EndpointState::Unverified
                    }
                    _ => EndpointState::Unreachable,
                }
            })
            .or_insert(next);
    }
    states
}

fn resolved_plan(
    status: &ServiceStatus,
    target: &crate::targets::ComputeTarget,
) -> Result<(service::DeployPlan, String, Vec<String>), String> {
    let declared = &status.service;
    let (program, args) = if !declared.program.is_empty() {
        (declared.program.clone(), declared.args.clone())
    } else {
        let entry = crate::deploy::service_catalog::lookup(&declared.name)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "nothing declares what {} runs on {}",
                    declared.name, declared.host
                )
            })?;
        crate::deploy::service_catalog::resolve_entry(
            &entry,
            &crate::deploy::service_catalog::home_for(target),
            Some(&target.release_platform),
            &target.name,
        )
    };
    let unit = declared.unit_id();
    if unit.is_empty() {
        return Err(format!(
            "{} on {} has no stable unit identity",
            declared.name, declared.host
        ));
    }
    let label = unit.strip_suffix(".service").unwrap_or(unit);
    let plan = service::plan_deploy_labelled(&declared.name, label, &program, &args)
        .map_err(|error| error.to_string())?;
    Ok((plan, program, args))
}

async fn replace_declaration(
    existing: &ManagedService,
    mut corrected: ManagedService,
    program: String,
    args: Vec<String>,
) -> Result<bool, String> {
    corrected.name = existing.name.clone();
    corrected.host_heuristic = existing.host_heuristic.clone();
    corrected.source = existing.source.clone();
    corrected.managed_since = existing.managed_since.clone();
    corrected.onboarding = existing.onboarding.clone();
    corrected.program = program;
    corrected.args = args;
    if corrected == *existing {
        return Ok(false);
    }
    let mut document = crate::cli::registry::fetch_document()
        .await
        .map_err(|error| error.to_string())?;
    service::replace_service(&mut document, &corrected).map_err(|error| error.to_string())?;
    crate::cli::registry::push_document(&document)
        .await
        .map_err(|error| error.to_string())?;
    Ok(true)
}

async fn reconcile_observed(
    status: &ServiceStatus,
    target: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<(String, bool, String), String> {
    let (plan, program, args) = resolved_plan(status, target)?;
    let report = service::probe_service(target, status.service.unit_id(), runner)
        .await
        .map_err(|error| error.to_string())?;
    if !report.succeeded("probed") {
        return Err(format!(
            "endpoint responds, but the declared unit could not be inspected: {}",
            report.failure()
        ));
    }
    if report.unit_state != "loaded" {
        return Err(
            "endpoint responds, but the declared unit is not loaded; refusing to create a duplicate until its owning unit or process is identified"
                .to_string(),
        );
    }
    let mut corrected = service::record_from_report(
        &status.service.host,
        status.service.host_heuristic.as_deref(),
        &status.service.name,
        &report,
        &status.service.managed_since,
    );
    corrected.program = program.clone();
    corrected.args = args.clone();
    let running = service::inspect_process(target, &corrected, runner)
        .await
        .map_err(|error| error.to_string())?;
    if running.matches_process() != Some(true) {
        return Err(format!(
            "endpoint responds and unit {} is loaded, but ownership is not proven by its running program",
            plan.label
        ));
    }
    let changed = replace_declaration(&status.service, corrected, program, args).await?;
    let action = if changed { "adopted" } else { "confirmed" };
    Ok((
        action.to_string(),
        changed,
        format!(
            "responding endpoint has a loaded unit {} running its declared program",
            plan.label
        ),
    ))
}

async fn reconcile_unreachable(
    status: &ServiceStatus,
    target: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<(String, bool, String), String> {
    let (plan, program, args) = resolved_plan(status, target)?;
    let outcome = service::ensure_service(target, &plan, runner)
        .await
        .map_err(|error| error.to_string())?;
    if !outcome.succeeded() {
        return Err(format!(
            "ensure did not establish a running unit: {}",
            outcome.report.failure()
        ));
    }
    let corrected = service::record_from_ensure(
        &status.service.host,
        &status.service.name,
        &outcome,
        &status.service.managed_since,
    );
    let declaration_changed =
        replace_declaration(&status.service, corrected, program, args).await?;
    Ok((
        outcome.action.clone(),
        outcome.changed() || declaration_changed,
        format!(
            "unit and endpoint were absent; ensure completed in {} domain",
            outcome.domain_word()
        ),
    ))
}

async fn persist_report(
    store: &JobStorage,
    report: &ServiceReconcileReport,
) -> Result<(), StorageError> {
    let object = report.created_at.replace(':', "-");
    crate::autonomy::storage::write_json(
        store,
        &format!("{REPORT_PREFIX}/{object}.json"),
        report,
        true,
    )
    .await?;
    crate::autonomy::storage::write_json(store, LATEST_REPORT, report, false).await
}

async fn alert_transitions(
    previous: Option<&ServiceReconcileReport>,
    report: &ServiceReconcileReport,
) {
    let prior: BTreeMap<String, &ServiceReconcileOutcome> = previous
        .map(|previous| {
            previous
                .outcomes
                .iter()
                .map(|outcome| (outcome.key(), outcome))
                .collect()
        })
        .unwrap_or_default();
    for outcome in report
        .outcomes
        .iter()
        .filter(|outcome| outcome.needs_alert())
    {
        let repeated = prior.get(&outcome.key()).is_some_and(|old| {
            old.classification == outcome.classification && old.detail == outcome.detail
        });
        if repeated {
            continue;
        }
        let message = format!(
            "Stado service reconciliation {} on {}: {}",
            outcome.service, outcome.host, outcome.detail
        );
        alerts::send_alert(
            crate::config::alerts_topic(),
            &message,
            "Stado could not reconcile a declared service",
        )
        .await;
    }
}

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
        if status.state == service::STATE_UNKNOWN {
            summary.unknown += 1;
            outcomes.push(ServiceReconcileOutcome {
                host: status.service.host.clone(),
                service: status.service.name.clone(),
                unit: status.service.unit_id().to_string(),
                beacon_state: status.state.clone(),
                endpoint_state: "not-used".to_string(),
                classification: "unknown".to_string(),
                action: "none".to_string(),
                changed: false,
                detail: status.detail.clone(),
            });
            continue;
        }
        if status.state != service::STATE_MISSING {
            continue;
        }
        summary.missing += 1;
        let endpoint = endpoints
            .get(&status.service.name)
            .copied()
            .unwrap_or(EndpointState::Absent);
        let mut outcome = ServiceReconcileOutcome {
            host: status.service.host.clone(),
            service: status.service.name.clone(),
            unit: status.service.unit_id().to_string(),
            beacon_state: status.state.clone(),
            endpoint_state: endpoint.word().to_string(),
            classification: String::new(),
            action: "none".to_string(),
            changed: false,
            detail: String::new(),
        };

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
        let planned_action = match endpoint {
            EndpointState::Observed => "adopt",
            EndpointState::Unreachable => "ensure",
            EndpointState::Unverified | EndpointState::Absent => {
                outcome.classification = "endpoint_unverified".to_string();
                outcome.detail = "unit is missing, but endpoint absence was not proven".to_string();
                summary.blocked += 1;
                outcomes.push(outcome);
                continue;
            }
        };
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
            outcome.classification = "lease_blocked".to_string();
            outcome.detail = "another reconciler owns this service mutation".to_string();
            summary.blocked += 1;
            outcomes.push(outcome);
            continue;
        };
        mutations += 1;
        let mut result = match endpoint {
            EndpointState::Observed => reconcile_observed(&status, &target, &runner).await,
            EndpointState::Unreachable => reconcile_unreachable(&status, &target, &runner).await,
            EndpointState::Unverified | EndpointState::Absent => unreachable!(),
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
                } else if error.starts_with("nothing declares") {
                    "declaration_incomplete".to_string()
                } else {
                    "repair_failed".to_string()
                };
                outcome.detail = error.clone();
                summary.failures += 1;
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
