//! Coordinator daemon — Rust port of `stado/coordinator.py`.
//!
//! The provider-neutral scheduling tick runs as a long-lived local process,
//! with no Cloud Run, Cloud Scheduler or Python control-plane dependency.
//!
//! The named coordinator entry supplies runtime cadence and identity only.
//! Queue/object state always comes from [`crate::queue::JobStorage::new`],
//! governed by Stado deployment config (`WC_STORAGE_BACKEND` plus its primary
//! and backup locators). A legacy registry `state_uri` is metadata and can
//! never override provider, backend, account, container or bucket.
//!
//! Cloud Function parity: `stado/cloud_function/main.py::monitor_jobs`
//! composes the SAME tick (fire due schedules -> normalize sizing ->
//! reap expired worker leases -> makespan assign -> per provider
//! check/reap/schedule -> run reaper -> billing collect) and needs no
//! separate port — [`run_tick`] is the single implementation. The lease
//! reaper is the tick's provider-neutral addition: it recovers `running/`
//! records whose worker died and queued pins naming silent workers, which
//! the per-cloud-provider monitor arms structurally cannot cover on a
//! local/box-only fleet. Credentials for both deployment shapes are resolved from
//! Skarbiec; the remaining deployment-specific difference is the box-owner
//! default (Cloud Function: "gcp-cloud-function"; daemon: hostname).
//!
//! Registry re-resolution runs every tick through
//! [`crate::targets::fetch_registry_remote`], which reads the configured
//! Stado store. The remote registry is the only authority for the
//! self-survival check — there is no local escape hatch.
//!
//! An unreadable primary (with no readable backup) is not interpreted as an
//! empty registry. The coordinator logs the storage failure and keeps
//! ticking; only a registry that was actually read may revoke the daemon.
//!
//! Release drift is resolved only from the exact configured Stado release
//! coordinate. No package-index channel participates in selection or update.
//! Billing: coordinator.py's daemon tick never collected billing (only
//!    the CF did). Per the port spec the tick includes the billing
//!    collector (fault-isolated, matching the CF), behind a flag so tests
//!    stay hermetic.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Utc};

use crate::config;
use crate::monitor::billing::collect_billing;
use crate::monitor::monitor::{check_running_jobs, reap_dead_agents, MonitorError};
use crate::monitor::reap::reap_terminal_runs;
use crate::providers::{get_provider, BoxProvider, Provider};
use crate::queue::{JobStorage, StorageError};
use crate::scheduler::dispatch::r#box::run_box_tick;
use crate::scheduler::makespan::{assign_jobs, repair_conflicting_pinned_assignments};
use crate::scheduler::scheduler::{
    schedule_queued_jobs, schedule_queued_jobs_routed, SchedulerError,
};
use crate::schedules::fire_due_schedules;
use crate::targets::{fetch_registry_remote, load_registry_auto, Coordinator};

/// `[tick] ...` — the coordinator's log prefix (Python `_log`).
fn log(msg: &str) {
    eprintln!("[tick] {msg}");
}

/// Tick failure.
#[derive(Debug, thiserror::Error)]
pub enum CoordinatorError {
    /// Storage failures from any tick phase.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Scheduler failures from `schedule_queued_jobs`.
    #[error(transparent)]
    Scheduler(#[from] SchedulerError),
    /// Monitor failures from `check_running_jobs` / `reap_dead_agents`.
    #[error(transparent)]
    Monitor(#[from] MonitorError),
}

/// One resolved provider arm of the tick. Box providers need their
/// concrete type for [`run_box_tick`], so they cannot hide behind
/// `Arc<dyn Provider>` here.
pub enum ResolvedProvider {
    /// A cloud VM provider (gcp/aws/azure): check + reap + schedule.
    Cloud {
        /// Provider name from `WC_PROVIDERS` (also the reaper `kind`).
        name: String,
        /// The provider client.
        provider: Arc<dyn Provider>,
    },
    /// A Box provider (box/box-ascii): the lease state machine tick.
    Box {
        /// Provider name from `WC_PROVIDERS`.
        name: String,
        /// The concrete box provider.
        provider: Arc<BoxProvider>,
    },
}

/// Pick the coordinator entry: explicit name or host-placement selector, or
/// the active one, from the configured Stado registry with bundled fallback.
async fn resolve_coordinator(target: Option<&str>) -> Result<Coordinator, String> {
    let registry = load_registry_auto().await.map_err(|exc| exc.to_string())?;
    if let Some(target) = target {
        return registry
            .lookup_coordinator_selector(target)
            .cloned()
            .ok_or_else(|| format!("coordinator selector '{target}' not found in registry"));
    }
    let active: Vec<&Coordinator> = registry.coordinators.iter().filter(|c| c.active).collect();
    if active.is_empty() {
        return Err(
            "no active coordinator in registry. Set active=true on one entry \
             or pass --target NAME explicitly."
                .into(),
        );
    }
    if active.len() > 1 {
        let names = active
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "multiple active coordinators ({names}); set active=true on exactly one"
        ));
    }
    Ok(active[0].clone())
}

/// `platform.node()` — the daemon-side default box-tick owner (Python
/// `os.uname().nodename`). Same approach as queue/submit.rs.
fn nodename() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return name;
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_default()
}

/// Internal scheduler-map key for the Azure agent grant. This deliberately is
/// not an environment-variable name and is never eligible for startup-script
/// substitution; the Azure provider consumes it only as protected settings.
pub(crate) const AZURE_AGENT_PROTECTED_GRANT: &str = "stado.protected-settings.azure-agent-grant";

/// Base64-encoded scoped workload grant projected only into non-Azure agent
/// templates. Azure receives the same grant through protected settings.
pub(crate) const AGENT_WORKLOAD_GRANT_B64: &str = "STADO_AGENT_SKARBIEC_GRANT_B64";

/// Validate the dedicated remote workload grant and return its opaque token.
///
/// The grant exposes only the configured workload-secret items. Azure receives
/// it through encrypted protected settings; other cloud startup templates
/// materialize the same scoped token into a root-only tmpfs file.
pub(crate) async fn agent_workload_grant() -> Result<Option<String>, crate::skarbiec::SkarbiecError>
{
    use crate::skarbiec::SkarbiecError;

    let remote_agents = config::wc_providers().iter().any(|name| {
        crate::capabilities::execution_adapter(name)
            .is_some_and(|adapter| adapter != crate::capabilities::ExecutionAdapter::Local)
    });
    if !remote_agents {
        return Ok(None);
    }
    let url = config::agent_skarbiec_url();
    if url.is_empty() {
        return Err(SkarbiecError::Deployment(
            "WC_AGENT_SKARBIEC_URL is required for remote workload agents; set it to an HTTPS \
             Skarbiec endpoint reachable from every agent VM"
                .to_string(),
        ));
    }
    if !url.starts_with("https://") {
        return Err(SkarbiecError::Deployment(format!(
            "WC_AGENT_SKARBIEC_URL={url:?} is not HTTPS; a remote workload grant must never \
             cross plaintext HTTP"
        )));
    }
    let consumer = config::agent_skarbiec_consumer();
    if consumer.is_empty()
        || !consumer.ends_with("-agent")
        || matches!(
            consumer,
            "stado-control-plane" | "stado-local-agent" | "stado-azure-agent"
        )
    {
        return Err(SkarbiecError::Deployment(format!(
            "WC_AGENT_SKARBIEC_CONSUMER={consumer:?} must be a scoped remote workload \
             identity ending in -agent and distinct from control-plane/legacy identities"
        )));
    }
    let token_file = config::agent_skarbiec_token_file();
    if token_file.is_empty() {
        return Err(SkarbiecError::Deployment(
            "WC_AGENT_SKARBIEC_TOKEN_FILE is required; use an owner-only workload grant"
                .to_string(),
        ));
    }
    let agent_token = crate::skarbiec::read_grant(token_file)?;
    let agent_vault = crate::skarbiec::Client::new(url, consumer, token_file)?;
    let mut visible: Vec<String> = agent_vault
        .list_items()
        .await?
        .into_iter()
        .map(|item| item.id)
        .collect();
    visible.sort();
    let mut expected = config::agent_skarbiec_items().to_vec();
    expected.sort();
    expected.dedup();
    if expected
        .iter()
        .any(|item| matches!(item.as_str(), "stado-aws" | "stado-azure" | "stado-gcp"))
    {
        return Err(SkarbiecError::Deployment(
            "agent.skarbiec.items must not contain cloud-provider credential items".to_string(),
        ));
    }
    for reference in config::agent_skarbiec_secret_fields() {
        let Some((item, field)) = reference.split_once('#') else {
            return Err(SkarbiecError::Deployment(format!(
                "agent.skarbiec.secret_fields entry {reference:?} must be item#field"
            )));
        };
        if item.is_empty()
            || field.is_empty()
            || !expected.iter().any(|configured| configured == item)
        {
            return Err(SkarbiecError::Deployment(format!(
                "agent.skarbiec.secret_fields entry {reference:?} is not covered by agent.skarbiec.items"
            )));
        }
    }
    if visible != expected {
        return Err(SkarbiecError::Deployment(format!(
            "consumer {consumer:?} can list {visible:?}; the remote workload grant must expose \
             exactly the configured workload-secret items"
        )));
    }
    Ok(Some(agent_token))
}

/// Resolve the dedicated workload grant needed by agent dispatch.
///
/// Azure consumes the raw token only through protected settings. Other remote
/// providers receive a base64 projection for root-only tmpfs materialization;
/// the renderer never logs the rendered script or any secret value.
pub(crate) async fn secrets_from_skarbiec(
) -> Result<BTreeMap<String, String>, crate::skarbiec::SkarbiecError> {
    let mut secrets = BTreeMap::new();
    if let Some(agent_token) = agent_workload_grant().await? {
        let mut azure = false;
        let mut inline = false;
        for provider in config::wc_providers() {
            match crate::capabilities::execution_adapter(provider) {
                Some(crate::capabilities::ExecutionAdapter::Azure) => azure = true,
                Some(crate::capabilities::ExecutionAdapter::Local) | None => {}
                Some(_) => inline = true,
            }
        }
        if azure {
            secrets.insert(AZURE_AGENT_PROTECTED_GRANT.to_string(), agent_token.clone());
        }
        if inline {
            secrets.insert(
                AGENT_WORKLOAD_GRANT_B64.to_string(),
                base64::engine::general_purpose::STANDARD.encode(agent_token.as_bytes()),
            );
        }
    }
    Ok(secrets)
}

/// Resolve `WC_PROVIDERS` into tick arms. "local" is skipped (device-local
/// agents claim assigned jobs directly; there is no cloud VM lifecycle to
/// schedule or reap for that provider). A constructor failure is logged
/// and skipped so a misconfigured provider never blocks the primary one
/// (Python wraps the box arm in try/except; the cloud arms construct
/// lazily and cannot fail here).
pub fn resolve_providers() -> Vec<ResolvedProvider> {
    let mut out = Vec::new();
    for name in config::wc_providers() {
        let Some(variant) =
            crate::capabilities::variant(crate::capabilities::RuntimeFacet::Compute, name)
        else {
            log(&format!(
                "provider {name} tick failed: capability is not registered"
            ));
            continue;
        };
        match variant.adapter {
            crate::capabilities::RuntimeAdapter::Compute(
                crate::capabilities::ComputeAdapter::ExistingHost,
            ) => continue,
            crate::capabilities::RuntimeAdapter::Compute(
                crate::capabilities::ComputeAdapter::Box,
            ) => match BoxProvider::from_env() {
                Ok(provider) => out.push(ResolvedProvider::Box {
                    name: variant.id.to_string(),
                    provider: Arc::new(provider),
                }),
                Err(exc) => log(&format!("provider {} tick failed: {exc}", variant.id)),
            },
            crate::capabilities::RuntimeAdapter::Compute(
                crate::capabilities::ComputeAdapter::Gcp
                | crate::capabilities::ComputeAdapter::Aws
                | crate::capabilities::ComputeAdapter::Azure,
            ) => match get_provider(variant.id) {
                Ok(provider) => out.push(ResolvedProvider::Cloud {
                    name: variant.id.to_string(),
                    provider,
                }),
                Err(exc) => log(&format!("provider {} tick failed: {exc}", variant.id)),
            },
            _ => log(&format!(
                "provider {} tick failed: no coordinator adapter",
                variant.id
            )),
        }
    }
    out
}

pub(crate) async fn run_autonomy_once(
    store: &JobStorage,
    providers: &[ResolvedProvider],
    mut policy: crate::autonomy::AutonomyPolicy,
    log: &dyn Fn(&str),
) -> Result<(), StorageError> {
    let now = Utc::now();
    let control = crate::autonomy::storage::load_control(store).await?;
    let circuit_open = control.circuit_open_at(now);
    if circuit_open {
        log(&format!(
            "autonomy circuit breaker open until {} after {} consecutive mutation failures: {}",
            control.circuit_open_until.as_deref().unwrap_or("unknown"),
            control.consecutive_mutation_failures,
            control.last_mutation_error.as_deref().unwrap_or("unknown"),
        ));
    }
    policy.emergency_paused |= control.emergency_paused || circuit_open;
    let prices_path = "state/autonomy/cost/prices.json";
    let prices: crate::autonomy::cost::PriceBook = match crate::autonomy::storage::read_json::<
        crate::autonomy::cost::PriceBook,
    >(store, prices_path)
    .await?
    {
        Some(book)
            if timestamp_fresh(
                &book.created_at,
                policy.freshness.pricing_max_age_seconds,
                now,
            ) =>
        {
            book
        }
        _ => crate::autonomy::cost::refresh_prices(&policy).await,
    };
    let cached = crate::autonomy::storage::load_latest_inventory(store).await?;
    let mut inventory = match cached {
        Some(snapshot)
            if policy.mode == crate::autonomy::AutonomyMode::Report
                && timestamp_fresh(
                    &snapshot.created_at,
                    policy.freshness.inventory_max_age_seconds,
                    now,
                ) =>
        {
            snapshot
        }
        _ => crate::autonomy::inventory::collect(store).await?,
    };
    let prior_snapshot_id = inventory.snapshot_id.clone();
    crate::autonomy::cost::enrich_inventory(&mut inventory, &prices);
    crate::autonomy::inventory::reseal(&mut inventory)?;
    if inventory.snapshot_id != prior_snapshot_id
        || crate::autonomy::storage::load_latest_inventory(store)
            .await?
            .is_none_or(|latest| latest.snapshot_id != inventory.snapshot_id)
    {
        crate::autonomy::storage::publish_inventory(store, &inventory).await?;
    }
    let budget_allocation = crate::autonomy::cost::build_allocation(store, &inventory).await?;
    let budget_billing = crate::autonomy::cost::load_billing_snapshot(store).await?;
    let budget_forecast =
        crate::autonomy::cost::forecast(&budget_allocation, &policy, budget_billing.as_ref(), now);
    let new_cloud_allowed = !budget_forecast.budget_exceeded;
    let hours_per_day = (crate::monitor::billing::SECONDS_PER_DAY
        / crate::monitor::billing::SECONDS_PER_HOUR) as f64;
    let daily_hourly_limit = policy.budgets.daily_usd.map(|limit| limit / hours_per_day);
    let hourly_limit = match (policy.budgets.hourly_usd, daily_hourly_limit) {
        (Some(hourly), Some(daily)) => Some(hourly.min(daily)),
        (Some(hourly), None) => Some(hourly),
        (None, Some(daily)) => Some(daily),
        (None, None) => None,
    };
    let mut new_cloud_hourly_budget_usd =
        hourly_limit.map(|limit| (limit - budget_forecast.current_hourly_usd).max(f64::default()));
    let mut new_cloud_cost_budget_usd = policy
        .budgets
        .monthly_usd
        .map(|limit| (limit - budget_forecast.end_of_month_usd).max(f64::default()));
    if !new_cloud_allowed || policy.mode != crate::autonomy::AutonomyMode::EnforceOwned {
        new_cloud_hourly_budget_usd = Some(f64::default());
        new_cloud_cost_budget_usd = Some(f64::default());
    }
    if !new_cloud_allowed {
        log(&format!(
            "autonomy budget guard: hourly overrun ${:.2}, daily overrun ${:.2}, monthly overrun ${:.2}; new cloud placement blocked",
            budget_forecast.hourly_overrun_usd,
            budget_forecast.daily_overrun_usd,
            budget_forecast.projected_overrun_usd,
        ));
    }

    if policy.mode != crate::autonomy::AutonomyMode::Report && inventory.complete {
        let cloud: Vec<(String, Arc<dyn Provider>)> = providers
            .iter()
            .filter_map(|provider| match provider {
                ResolvedProvider::Cloud { name, provider } => {
                    Some((name.clone(), Arc::clone(provider)))
                }
                ResolvedProvider::Box { .. } => None,
            })
            .collect();
        let placement = crate::autonomy::optimizer::plan_queued(
            store,
            &cloud,
            &policy,
            &prices,
            &inventory.snapshot_id,
            log,
            new_cloud_hourly_budget_usd,
            new_cloud_cost_budget_usd,
        )
        .await?;
        log(&format!(
            "autonomy placement: considered={} decided={} changed={} blocked={}",
            placement.considered_jobs,
            placement.decided_jobs,
            placement.changed_jobs,
            placement.no_eligible_target
        ));
    } else if policy.mode != crate::autonomy::AutonomyMode::Report {
        log("autonomy placement blocked: inventory is incomplete");
    }

    let fingerprint = crate::cli::resources::planner::configuration_fingerprint()
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let reconciliation =
        crate::autonomy::reconciler::reconcile(store, &inventory, &policy, &fingerprint, log)
            .await?;
    if reconciliation.findings > usize::default() {
        log(&format!(
            "autonomy reconciliation: findings={} actions={} executed={}",
            reconciliation.findings, reconciliation.automatic_actions, reconciliation.executed
        ));
    }
    crate::autonomy::service_reconciler::reconcile(store, &policy, log).await?;
    let advice =
        crate::autonomy::advisor::publish_recommendations(store, &inventory, &policy, now).await?;
    if advice.rightsizing > usize::default()
        || advice.schedules > usize::default()
        || advice.storage_lifecycle > usize::default()
        || advice.network > usize::default()
        || advice.commitments > usize::default()
    {
        log(&format!(
            "autonomy advice: rightsizing={} schedules={} storage={} network={} commitments={}",
            advice.rightsizing,
            advice.schedules,
            advice.storage_lifecycle,
            advice.network,
            advice.commitments
        ));
    }

    let allocation = crate::autonomy::cost::build_allocation(store, &inventory).await?;
    let billing = crate::autonomy::cost::load_billing_snapshot(store).await?;
    let forecast = crate::autonomy::cost::forecast(&allocation, &policy, billing.as_ref(), now);
    let anomalies = crate::autonomy::cost::detect_anomalies(&allocation, &inventory, &forecast);
    crate::autonomy::cost::persist_reports(store, &prices, &allocation, &forecast, &anomalies)
        .await?;
    let outcomes = crate::autonomy::cost::measure_outcomes(store).await?;
    if outcomes.feedback_written > usize::default() || outcomes.savings_measured > usize::default()
    {
        log(&format!(
            "autonomy outcomes: feedback={} savings-measured={}",
            outcomes.feedback_written, outcomes.savings_measured
        ));
    }
    let savings = crate::autonomy::storage::list_savings(store).await?;
    let measurements = crate::autonomy::storage::list_savings_measurements(store).await?;
    let savings_summary =
        crate::autonomy::cost::summarize_savings_with_measurements(&savings, &measurements);
    crate::autonomy::storage::write_json(
        store,
        "state/autonomy/cost/savings.json",
        &savings_summary,
        false,
    )
    .await?;
    let lifecycle = crate::autonomy::lifecycle::enforce(store, &policy, now).await?;
    if lifecycle.deleted > usize::default() {
        log(&format!(
            "autonomy lifecycle: deleted={} bytes={} capped={}",
            lifecycle.deleted, lifecycle.deleted_bytes, lifecycle.capped
        ));
    }
    Ok(())
}

fn timestamp_fresh(raw: &str, max_age_seconds: u64, now: chrono::DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(raw)
        .map(|stamp| {
            now.signed_duration_since(stamp.with_timezone(&Utc))
                .num_seconds()
        })
        .is_ok_and(|age| {
            age >= i64::default() && age <= i64::try_from(max_age_seconds).unwrap_or(i64::MAX)
        })
}

/// One scheduling cycle across every provider (Python
/// `coordinator._run_tick` + the CF's billing tail).
///
/// Each provider gets its own check_running_jobs + schedule_queued_jobs
/// pass; the queue is shared (state lives in JobStorage), so a
/// pin_to_provider job lands wherever its provider field points and an
/// unpinned job is offered to whichever provider claims first.
///
/// `with_billing` runs the billing-credits collector at the end (the CF
/// behavior; coordinator.py's daemon never billed). Tests pass `false`
/// to stay hermetic — the collector talks to BigQuery/ARM.
pub async fn run_tick(
    store: &JobStorage,
    secrets: &BTreeMap<String, String>,
    providers: &[ResolvedProvider],
    with_billing: bool,
    log: &dyn Fn(&str),
) -> Result<i64, CoordinatorError> {
    if let Err(exc) = config::refresh_model_policy(store).await {
        log(&format!(
            "model policy refresh failed; retaining last good policy: {exc}"
        ));
    }
    // Fire recurring (cron) schedules FIRST so any job submitted this tick
    // is visible to the assignment + dispatch passes below, instead of
    // waiting a full interval_seconds to be picked up.
    let n_fired = fire_due_schedules(store, log, Utc::now()).await?;
    if n_fired > 0 {
        log(&format!("schedules: fired {n_fired} due schedule(s)"));
    }
    // Coordinator-authoritative sizing: re-zero any queued job whose model
    // has no measured peak (stamp the measured peak if one exists) BEFORE
    // assignment. A pre-0.4.237 agent that requeues a job writes the old
    // hardcoded estimate_gpu_memory value back; makespan's assigned_to-only
    // write then preserves it. Correcting it here each tick makes the
    // coordinator the single sizing authority instead of waiting for
    // fleet-wide drift.
    let n_sized = crate::sizing::global()
        .normalize_queue_sizing(store, log)
        .await?;
    if n_sized > 0 {
        log(&format!(
            "sizing: corrected {n_sized} stale queue gpu_mem_gb values"
        ));
    }
    // Phantom-job reaper: a worker that dies mid-job leaves its running/
    // record behind, and the per-cloud-provider monitor arms above never
    // run on a fleet with no cloud provider (or one whose API is down) —
    // the queue then reports capacity that does not exist and the job
    // waits forever. This provider-neutral pass requeues the job once its
    // worker lease (status/<job_id>/heartbeat, HEARTBEAT_STALE_MINUTES)
    // expires, fails it with a stored reason on the second expiry, and
    // clears queued assignments naming silent workers. It runs BEFORE
    // assignment and dispatch so recovered jobs are claimable this tick.
    let reaped = crate::queue::reaper::reap_expired_leases(store, log).await?;
    if reaped.requeued > 0 || reaped.failed > 0 || reaped.assignments_cleared > 0 {
        log(&format!(
            "lease-reaper: requeued {} phantom job(s), failed {} on second expiry, \
             cleared {} silent-worker assignment(s)",
            reaped.requeued, reaped.failed, reaped.assignments_cleared
        ));
    }
    let autonomy_requires_routing = match crate::autonomy::storage::load_policy(store).await {
        Ok(policy) => {
            let routed = policy.mode != crate::autonomy::AutonomyMode::Report;
            if let Err(error) = run_autonomy_once(store, providers, policy, log).await {
                log(&format!("autonomy tick degraded: {error}"));
            }
            routed
        }
        Err(error) => {
            log(&format!(
                "autonomy policy unreadable; fail-closing unpinned dispatch: {error}"
            ));
            true
        }
    };
    if !autonomy_requires_routing {
        // Report mode preserves the legacy makespan matcher. Enforced
        // autonomy has already selected a provider/consumer atomically;
        // running this matcher afterwards would overwrite that decision.
        let n_assigned = assign_jobs(store, log).await?;
        if n_assigned > usize::default() {
            log(&format!(
                "assignment: matched {n_assigned} queued jobs to agents"
            ));
        }
    }
    let n_pin_repairs = repair_conflicting_pinned_assignments(store, log).await?;
    if n_pin_repairs > 0 {
        log(&format!(
            "routing: repaired {n_pin_repairs} conflicting host-pinned assignments"
        ));
    }
    let mut total: i64 = 0;
    for arm in providers {
        match arm {
            ResolvedProvider::Box { name, provider } => {
                let owner = std::env::var("WC_COORDINATOR_ID").unwrap_or_else(|_| nodename());
                match run_box_tick(store, provider, &owner).await {
                    Ok(n) => total += n,
                    Err(exc) => log(&format!("provider {name} tick failed: {exc}")),
                }
            }
            ResolvedProvider::Cloud { name, provider } => {
                check_running_jobs(store, provider.as_ref()).await?;
                let reaped = reap_dead_agents(store, provider.as_ref(), name).await?;
                if reaped > 0 {
                    log(&format!("{name}: reaped {reaped} dead-agent VM(s)"));
                }
                total += if autonomy_requires_routing {
                    schedule_queued_jobs_routed(store, provider.as_ref(), name, secrets).await?
                } else {
                    schedule_queued_jobs(store, provider.as_ref(), name, secrets).await?
                };
            }
        }
    }
    // By-run reaper: drop per-job blobs once a run is fully terminal so
    // completed/+failed/ stop accumulating thousands of orphaned records.
    // Capped per tick to bound work on a large backlog.
    let summary = reap_terminal_runs(store, config::RUN_REAP_PER_TICK).await?;
    if summary.reaped_runs > 0 {
        log(&format!(
            "run-reaper: reaped {} run(s), deleted {} job blob(s)",
            summary.reaped_runs, summary.deleted_jobs
        ));
    }
    if with_billing {
        // Billing-credits collector. Global (not per-provider), runs last
        // and is fully fault-isolated internally: each source's exact error
        // is captured into the JSON blob (and the upload itself only logs),
        // so a broken collector never aborts the dispatch tick that the
        // drain depends on (CF behavior; Python coordinator.py's daemon
        // never billed).
        collect_billing(store).await;
    }
    Ok(total)
}

/// Coordinator daemon entry point (Python `coordinator.run`). Returns the
/// process exit code; `Err` is a SystemExit-style fatal message.
pub async fn run(target: Option<&str>, once: bool) -> Result<i32, String> {
    let coord = resolve_coordinator(target).await?;
    if coord.runtime == "gcp_cloud_function" {
        log(&format!(
            "coordinator '{}' runtime=gcp_cloud_function: tick is driven by \
             Cloud Scheduler, this daemon is a no-op. Use --target to point \
             at a runtime=daemon entry instead.",
            coord.name
        ));
        return Ok(0);
    }

    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let interval = coord.interval_seconds.max(15) as u64;
    log(&format!(
        "coordinator '{}' runtime={} interval={interval}s storage={} \
         registry_state_uri_metadata={:?}",
        coord.name,
        coord.runtime,
        store.backend_name(),
        coord.state_uri
    ));

    let secrets = secrets_from_skarbiec()
        .await
        .map_err(|err| err.to_string())?;
    loop {
        if !config::stado_api_url().is_empty()
            && !config::stado_release_version().is_empty()
            && !config::stado_release_platform().is_empty()
        {
            let mut update_log = |message: &str| log(message);
            match crate::self_update::self_update(&mut update_log).await {
                Ok(crate::self_update::UpdateOutcome::Updated { from, to }) => {
                    log(&format!(
                        "coordinator self-update installed {from} -> {to}; re-executing"
                    ));
                    let exc = crate::self_update::reexec();
                    log(&format!(
                        "coordinator self-update re-exec failed; continuing old process image: \
                         {exc}"
                    ));
                }
                Ok(crate::self_update::UpdateOutcome::UpToDate { .. }) => {}
                Err(exc) => log(&format!(
                    "coordinator self-update failed; continuing current version: {exc}"
                )),
            }
        }
        // Re-resolve the coordinator entry from the registry each tick. The
        // initial resolve at process start captures the entry once and
        // never re-checks; if an operator pushes a new registry that
        // removes/renames the entry to stop a racing daemon, the running
        // process keeps reaping VMs forever using the cached entry.
        // Confirmed live 2026-05-15: a stale mac mini daemon kept deleting
        // fresh-heartbeat Llama/Qwen3 VMs for 4+ hours after the registry
        // entry was removed because pip drift never fired (the daemon was
        // already on the latest published version). Re-resolving each tick
        // means a registry change takes effect within one interval_seconds
        // without depending on a new release being published.
        // The canonical registry is read from configured Stado storage and is
        // the only self-survival authority. A registry we could not read is
        // not an authority at all, even when primary reads are failing over.
        if let Some(target) = target {
            let survival = fetch_registry_remote().await;
            // Exit ONLY when a registry we actually READ omits the entry.
            // An unreachable store says nothing about whether the operator
            // revoked us — see `targets::RegistryFetchError`.
            if matches!(&survival, Ok(registry) if registry.lookup_coordinator_selector(target).is_none())
            {
                log(&format!(
                    "coordinator '{target}' not in the canonical registry; exiting. \
                     Operator removed/renamed the entry — daemon stops here so \
                     launchd/supervisor backs off and stale code stops issuing \
                     cloud-resource mutations."
                ));
                return Ok(0);
            }
            if let Err(exc) = survival {
                log(&format!(
                    "canonical registry unreachable ({exc}); SKIPPING the \
                     self-survival check for coordinator '{target}' and \
                     CONTINUING. A storage outage must never mass-terminate \
                     the fleet — the kill switch fires only against a \
                     registry that was actually read."
                ));
            }
        }
        let providers = resolve_providers();
        let n = run_tick(&store, &secrets, &providers, true, &log)
            .await
            .map_err(|exc| exc.to_string())?;
        log(&format!("tick scheduled={n}"));
        // Record the queue namespace this coordinator serves into the
        // canonical registry so submitters can refuse an ambient namespace
        // the fleet never claims from (the 2026-08-19 silent-stall: a job
        // submitted under the operator's ambient namespace sat unclaimed
        // for hours). Ok(false) — already current, empty namespace, or a
        // lost CAS race — needs no line.
        if let Err(exc) =
            crate::targets::record_fleet_queue_namespace(config::wc_stado_storage_namespace())
                .await
        {
            log(&format!("fleet queue namespace record failed: {exc}"));
        }
        // Native-build poller: watch registry build recipes for new commits
        // and enqueue build jobs. Self-rate-limited to one pass per minute;
        // per-recipe failures are logged inside, never raised.
        crate::scheduler::builds::poll_build_recipes(&log).await;
        match crate::queue::copy::replicate_configured_backup().await {
            Ok(Some(report)) if report.is_clean() => log("disaster-recovery replication clean"),
            Ok(Some(report)) => log(&format!(
                "disaster-recovery replication incomplete: {} object(s) failed",
                report.failed()
            )),
            Ok(None) => {}
            Err(exc) => log(&format!("disaster-recovery replication failed: {exc}")),
        }
        if once {
            return Ok(0);
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}
