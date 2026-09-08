//! The tick itself — the single ordered composition of every pass.

use std::collections::BTreeMap;

use chrono::Utc;

use crate::config;
use crate::coordinator::nodename;
use crate::monitor::billing::collect_billing;
use crate::monitor::monitor::{check_running_jobs, reap_dead_agents, MonitorError};
use crate::monitor::reap::reap_terminal_runs;
use crate::queue::{JobStorage, StorageError};
use crate::scheduler::dispatch::r#box::run_box_tick;
use crate::scheduler::makespan::{assign_jobs, repair_conflicting_pinned_assignments};
use crate::scheduler::scheduler::{
    schedule_queued_jobs, schedule_queued_jobs_routed, SchedulerError,
};
use crate::schedules::fire_due_schedules;

use super::autonomy::run_autonomy_once;
use super::providers::ResolvedProvider;

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
    let cancellations = store.settle_queued_cancellations().await?;
    if cancellations > 0 {
        log(&format!(
            "cancellation: finalized {cancellations} queued request(s)"
        ));
    }
    if let Err(exc) = config::refresh_model_policy(store).await {
        log(&format!(
            "model policy refresh failed; retaining last good policy: {exc}"
        ));
    }
    // Fire recurring (cron) schedules FIRST so any job submitted this tick is
    // visible to the assignment + dispatch passes below, instead of waiting a
    // full interval_seconds to be picked up. One malformed or concurrently
    // retired schedule occurrence is not allowed to suppress queue recovery:
    // on 2026-09-05 a missing Spis run manifest made launchd restart this
    // coordinator before it could reap a dead release worker on every tick.
    match fire_due_schedules(store, log, Utc::now()).await {
        Ok(n_fired) if n_fired > 0 => log(&format!("schedules: fired {n_fired} due schedule(s)")),
        Ok(_) => {}
        Err(error) => log(&format!(
            "schedules: reconciliation degraded; continuing queue recovery: {error}"
        )),
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
    // record behind, and the per-cloud-provider monitor arms above never run
    // on a fleet with no cloud provider (or one whose API is down). The
    // provider-neutral pass completes a stale release job when its already
    // durable receipt and archive verify, otherwise requeues once and fails
    // on the second expiry. It also clears queued assignments naming silent
    // workers. It runs BEFORE assignment and dispatch so recovered work is
    // visible in this tick.
    let reaped = crate::queue::reaper::reap_expired_leases(store, log).await?;
    if reaped.release_completions > 0
        || reaped.requeued > 0
        || reaped.failed > 0
        || reaped.assignments_cleared > 0
    {
        log(&format!(
            "lease-reaper: completed {} release job(s) from durable output, requeued {} \
             phantom job(s), failed {} on second expiry, cleared {} silent-worker \
             assignment(s)",
            reaped.release_completions, reaped.requeued, reaped.failed, reaped.assignments_cleared
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
    // Capped per tick to bound work on a large backlog. Cleanup is not the
    // scheduler: a manifest listed for cleanup can disappear between that
    // listing and the cleanup reader's legacy-migration read. The race is
    // reported, but it must not terminate the daemon after dispatch has
    // completed and cause launchd to restart it in a tight loop.
    match reap_terminal_runs(store, config::RUN_REAP_PER_TICK).await {
        Ok(summary) => {
            if summary.reaped_runs > 0 {
                log(&format!(
                    "run-reaper: reaped {} run(s), deleted {} job blob(s)",
                    summary.reaped_runs, summary.deleted_jobs
                ));
            }
            for refusal in &summary.refused_runs {
                log(&format!(
                    "run-reaper: retained run {} without cleanup: {}",
                    refusal.run_id, refusal.reason
                ));
            }
        }
        Err(error) => log(&format!("run-reaper: cleanup degraded: {error}")),
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
