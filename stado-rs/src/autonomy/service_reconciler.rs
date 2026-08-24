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

const LATEST_REPORT: &str = "state/autonomy/services/latest.json";
const REPORT_PREFIX: &str = "state/autonomy/services/runs";
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

/// Render the unit a repair would assert, through the one resolution chain
/// `service ensure` already uses: the host's own declaration, then the shipped
/// Wisent catalog, then the declaration bundled with this build. A second
/// resolution order here would let a repair install a different program than
/// an operator's `ensure` for the same name.
fn resolved_plan(
    status: &ServiceStatus,
    target: &crate::targets::ComputeTarget,
) -> Result<(service::DeployPlan, String, Vec<String>), String> {
    let declared = &status.service;
    let mut unit =
        crate::cli::service::unit_program(&target.name, &declared.name, None, &[], Some(declared))
            .map_err(|error| error.to_string())?;
    if unit.source == "catalog" {
        let entry = crate::deploy::service_catalog::CatalogService {
            name: declared.name.clone(),
            summary: String::new(),
            unit: unit.unit.clone(),
            program: unit.program.clone(),
            args: unit.args.clone(),
        };
        let (program, args) = crate::deploy::service_catalog::resolve_entry(
            &entry,
            &crate::deploy::service_catalog::home_for(target),
            Some(&target.release_platform),
            &target.name,
        );
        unit.program = program;
        unit.args = args;
    }
    let plan = match unit
        .unit
        .as_deref()
        .or_else(|| crate::cli::service::declared_label(declared))
    {
        Some(label) => {
            service::plan_deploy_labelled(&declared.name, label, &unit.program, &unit.args)
        }
        None => service::plan_deploy(&declared.name, &unit.program, &unit.args),
    }
    .map_err(|error| error.to_string())?;
    Ok((plan, unit.program, unit.args))
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

/// Repair the one unit whose death blinds every other repair.
///
/// A silent host beacon turns every service on that host `unknown`, and this
/// stage rightly refuses to mutate on unknown evidence — which would leave a
/// dead beacon dead forever, and with it the whole host unrepairable. The
/// beacon unit is the one exception: the evidence for "the beacon is down" is
/// the beacon's own absence, and the evidence that repair is possible is the
/// host channel answering. `ensure` restarts in place and never unloads, so a
/// beacon that is actually healthy but unheard is kicked, not destroyed.
///
/// The registry is only written for a registry-sourced declaration; a
/// recovery-sourced beacon stays owned by the fixed host-recovery program.
async fn reconcile_beacon(
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
            "beacon ensure did not establish a running unit: {}",
            outcome.report.failure()
        ));
    }
    let mut declaration_changed = false;
    if status.service.source == service::SOURCE_REGISTRY {
        let corrected = service::record_from_ensure(
            &status.service.host,
            &status.service.name,
            &outcome,
            &status.service.managed_since,
        );
        declaration_changed =
            replace_declaration(&status.service, corrected, program, args).await?;
    }
    Ok((
        outcome.action.clone(),
        outcome.changed() || declaration_changed,
        format!(
            "host beacon was silent; reasserted the beacon unit in the {} domain so evidence can resume",
            outcome.domain_word()
        ),
    ))
}

/// A declared unit the service directory says nothing about has no endpoint
/// to disprove, so "endpoint absence was not proven" would block its repair
/// forever. The host channel is the evidence instead: the unit is probed on
/// the box, a loaded unit must prove its live program before the declaration
/// is corrected, and only a unit the host itself reports absent is ensured.
async fn reconcile_undeclared(
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
            "the unit could not be inspected over the host channel: {}",
            report.failure()
        ));
    }
    if report.unit_state == "loaded" {
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
        match running.matches_process() {
            Some(true) => {
                let changed =
                    replace_declaration(&status.service, corrected, program, args).await?;
                let action = if changed { "adopted" } else { "confirmed" };
                return Ok((
                    action.to_string(),
                    changed,
                    format!(
                        "beacon omitted unit {}, but the host reports it loaded and running its declared program",
                        plan.label
                    ),
                ));
            }
            // `Some(false)` covers two different worlds and only one is a
            // conflict. A process executing a binary the unit never declared
            // is unknown ownership and stays refused. A process executing the
            // declared binary that was REWRITTEN after the process started is
            // the four-day stale-agent incident, and the in-place kick below
            // is precisely its repair.
            Some(false) => {
                let same_binary = running
                    .running_binary()
                    .is_some_and(|binary| binary == running.declared || binary == running.resolved);
                if !same_binary {
                    return Err(format!(
                        "unit {} is loaded on the host but ownership is not proven by its running program",
                        plan.label
                    ));
                }
            }
            // the in-place `ensure` kick below is the repair, not a risk.
            None => {}
        }
    }
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
            "the host itself reported the unit absent; ensure completed in {} domain",
            outcome.domain_word()
        ),
    ))
}

/// A refused write must name the object and the boundary that refused it.
///
/// The object API authorizes a write by matching its key against the
/// configured namespace's prefix policies, and a key no policy covers comes
/// back `401 {"error":"unauthorized or non-immutable release write"}` — a
/// sentence naming neither the namespace, the prefix, nor the grant. The whole
/// autonomy layer writes under `autonomy/`, which no namespace policy declares,
/// so every run of this stage is refused on a deployment whose namespace does
/// not authorize that prefix. Diagnosing that from the bare 401 took twenty
/// minutes; the reconciliation verdict is worthless if the record of it cannot
/// be found, so the failure says exactly which object was refused.
async fn persist_report(
    store: &JobStorage,
    report: &ServiceReconcileReport,
) -> Result<(), StorageError> {
    let object = report.created_at.replace(':', "-");
    let run_path = format!("{REPORT_PREFIX}/{object}.json");
    for path in [run_path.as_str(), LATEST_REPORT] {
        let immutable = path != LATEST_REPORT;
        if let Err(error) =
            crate::autonomy::storage::write_json(store, path, report, immutable).await
        {
            return Err(StorageError::Other(format!(
                "service reconciliation ran but could not record it at {path}: {error}. The \
                 configured Stado storage namespace must authorize `put` on this key's prefix; \
                 `stado config show` reports the namespace and its prefix policies"
            )));
        }
    }
    Ok(())
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
                outcome.classification = "unknown".to_string();
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
            outcome.classification = "lease_blocked".to_string();
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
