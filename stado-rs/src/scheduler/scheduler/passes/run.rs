//! Pass three: the tick driver that runs the other two and dispatches.
//!
//! Maintenance gate -> quota read -> candidate window -> body reads ->
//! per-accel fairness share -> live local capacity -> local pack ->
//! agent-VM dispatch.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::Utc;

use crate::config;
use crate::models::Job;
use crate::providers::Provider;
use crate::queue::capacity;
use crate::queue::control;
use crate::queue::JobStorage;
use crate::scheduler::cost;
use crate::scheduler::dispatch::agent::{dispatch_agent_vms, AgentDispatchInputs};
use crate::scheduler::quota::get_available_instances;
use crate::scheduler::scheduler::support::pacing::dynamic_per_tick_cap;
use crate::scheduler::scheduler::support::reporting::{log, py_dict_i64, py_pairs_i64};
use crate::scheduler::scheduler::SchedulerError;

use super::local_pack::local_pack;
use super::prefilter::prefilter_candidates_with_routing;

/// Pick queued jobs that fit available GPU slots and cost caps; create
/// instances. Python `schedule_queued_jobs`.
pub async fn schedule_queued_jobs(
    store: &JobStorage,
    provider: &dyn Provider,
    provider_name: &str,
    secrets: &BTreeMap<String, String>,
) -> Result<i64, SchedulerError> {
    schedule_queued_jobs_inner(store, provider, provider_name, secrets, false).await
}

pub async fn schedule_queued_jobs_routed(
    store: &JobStorage,
    provider: &dyn Provider,
    provider_name: &str,
    secrets: &BTreeMap<String, String>,
) -> Result<i64, SchedulerError> {
    schedule_queued_jobs_inner(store, provider, provider_name, secrets, true).await
}

async fn schedule_queued_jobs_inner(
    store: &JobStorage,
    provider: &dyn Provider,
    provider_name: &str,
    secrets: &BTreeMap<String, String>,
    require_provider_pin: bool,
) -> Result<i64, SchedulerError> {
    // Maintenance-mode gate (queue::control — read that module for the
    // full semantics). A paused queue dispatches NOTHING: no quota read,
    // no instance created, no new cloud spend. The backlog is left exactly
    // as it is, because pausing is not cancelling, and jobs already in
    // running/ finish normally — which is what lets `stado queue drain
    // --wait` terminate. Re-read every tick so `stado queue resume` takes
    // effect on the next one.
    let queue_control = control::read(store).await?;
    if queue_control.paused {
        log(&format!(
            "Queue paused ({}); dispatching nothing",
            queue_control.pause_summary()
        ));
        return Ok(i64::default());
    }

    let available = get_available_instances(store, provider, provider_name).await?;
    log(&format!(
        "Available GPU instances by accelerator: {}",
        py_dict_i64(&available)
    ));

    if available.values().all(|count| *count == 0) {
        log("Provider quota allows no additional GPU instances");
        return Ok(0);
    }

    // Cap the listing in JobStorage so we never download more than we'd
    // dispatch this tick. queue/ holds 14k+ blobs after a big batch submit
    // and downloading every JSON blew the 60s function timeout. Pick by
    // GCS time_created ascending (FIFO) — anything past
    // _dynamic_per_tick_cap's ceiling × 8 wouldn't fit in this tick's
    // budget anyway.
    let window_budget = dynamic_per_tick_cap(1_000_000_000) as usize * 8;

    let blobs = store.list_blobs_with_meta("queue/").await?;
    let (candidates, skipped_no_quota) = prefilter_candidates_with_routing(
        &blobs,
        &available,
        provider_name,
        window_budget,
        require_provider_pin,
    );
    if skipped_no_quota > 0 {
        log(&format!(
            "window: skipped {skipped_no_quota} undispatchable (0-quota-accel) queued jobs"
        ));
    }
    let mut queued: Vec<Job> = Vec::new();
    for jid in &candidates {
        if let Some(j) = store.read_job("queue", jid).await? {
            queued.push(j);
        }
    }
    let now_utc = Utc::now();
    let full_queue_depth = queued.len() as i64;
    let per_tick_cap = dynamic_per_tick_cap(full_queue_depth);
    queued.truncate(per_tick_cap as usize * 8);
    // filter_already_done was disabled: HfApi.list_repo_files on the
    // 184k-file wisent-ai/activations repo takes 50+s, eating the 60s
    // function timeout before any dispatch fires. Wrapper still
    // short-circuits per-strategy on the box so the cost is only VM boot
    // for results-already-uploaded jobs.

    // Per-accelerator fairness: when a heterogeneous batch is queued
    // (e.g. T4 + A100-40 + A100-80 jobs all waiting), pure FIFO means the
    // first-submitted accel hogs every tick until its quota saturates
    // while other accels sit idle. Compute a soft per-accel per-tick share
    // so each accel makes progress concurrently. Round up so
    // distinct_accels=3 with cap=50 gives 17 each (the leftover 1 falls to
    // whichever accel comes first in the sorted queue). The pass after
    // this loop fills any remaining budget without per-accel limits, so we
    // don't underuse.
    let distinct_accels: BTreeSet<&str> = queued
        .iter()
        .map(|j| {
            if j.gpu_type.is_empty() {
                "_cpu"
            } else {
                j.gpu_type.as_str()
            }
        })
        .collect();
    let per_accel_share = if distinct_accels.is_empty() {
        per_tick_cap
    } else {
        let n = distinct_accels.len() as i64;
        (per_tick_cap + n - 1).div_euclid(n).max(1)
    };

    // Read live worker capacity. An accepting local worker with room for an
    // accelerator class is a free-hardware peer we should use before paying
    // for a fresh GCE VM. We track placements by accelerator so a job yielded
    // in this tick does not spend the same live resource twice.
    let consumer_caps = capacity::read_consumer_capacity(store).await?;
    let local_provider = [crate::capabilities::ProviderId::Local.as_str()];
    let local_free =
        capacity::total_available_accelerators(&consumer_caps, Some(local_provider.as_slice()));
    let local_vram_pool =
        capacity::consumers_by_free_vram(&consumer_caps, Some(local_provider.as_slice()));
    if !local_free.is_empty() {
        log(&format!(
            "Live local accelerator availability: {}",
            py_dict_i64(&local_free)
        ));
    }
    if !local_vram_pool.is_empty() {
        log(&format!(
            "Live local-agent free_vram_gb: {}",
            py_pairs_i64(&local_vram_pool)
        ));
    }

    let mut yield_targets = HashMap::new();
    if !local_vram_pool.is_empty() {
        let wt_table = cost::wall_time_table(&cost::collect_completed(store).await?);
        yield_targets = local_pack(&queued, &local_vram_pool, &wt_table, now_utc);
    }
    if per_tick_cap != config::MAX_SCHEDULE_PER_TICK {
        log(&format!(
            "Autoscale per-tick cap: {} -> {} (queue={})",
            config::MAX_SCHEDULE_PER_TICK,
            per_tick_cap,
            queued.len()
        ));
    }

    // Agent-mode dispatch: launch agent VMs that poll the queue and pack
    // jobs by VRAM. Replaces the per-job VM dispatch — per-VM concurrency
    // is now bounded by nvidia-smi readout, not a constant.
    let mut available = available;
    let mut accel_dispatched: BTreeMap<String, i64> = BTreeMap::new();
    let created = dispatch_agent_vms(
        AgentDispatchInputs {
            queued,
            yield_targets,
            available: &mut available,
            accel_dispatched: &mut accel_dispatched,
            per_accel_share,
            per_tick_cap,
            scheduled_so_far: 0,
        },
        store,
        crate::sizing::global(),
        provider,
        provider_name,
        secrets,
        now_utc,
    )
    .await?;
    Ok(created)
}
