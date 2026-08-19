//! Job scheduler: pick queued jobs and create instances.
//!
//! Port of `stado/scheduler/scheduler.py`. Routing rules:
//! - job.pin_to_provider=True + job.provider="local" -> only local agent claims
//! - job.pin_to_provider=True + job.provider=<X>     -> only provider X claims
//! - job.pin_to_provider=False (default)             -> any consumer with
//!   capacity can claim. The Cloud Function (this file) skips a job ONLY if
//!   its capacity cannot satisfy the job (no quota, or cost cap exceeds
//!   available SKU rate); the local agent then has a chance.
//!
//! Dispatch backoff:
//! A job whose create_instance call failed gets dispatch_attempts++ and a
//! last_dispatch_attempt timestamp. It is then skipped for a backoff window
//! that grows with attempt count. This prevents a wedged job (e.g. quota
//! exhausted in every zone) from slamming the API on every 3-min tick AND
//! gives the local agent a clean shot at the same job in the meantime.
//!
//! Deviation: Python defines a `_attempt` closure (the legacy 1-VM-per-job
//! dispatch path) that is never called — agent-mode dispatch
//! ([`dispatch::agent::dispatch_agent_vms`]) fully replaced it. The dead
//! closure is not ported; its per-job behaviors (empty-machine_type
//! failure guard, backoff accounting, no-preemptible policy) live on in
//! the bucketed agent dispatch.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Duration, Utc};

use crate::config;
use crate::models::Job;
use crate::providers::{Provider, ProviderError};
use crate::queue::capacity;
use crate::queue::control;
use crate::queue::{JobStorage, StorageError};
use crate::scheduler::cost;
use crate::scheduler::dispatch::agent::{dispatch_agent_vms, AgentDispatchInputs};
use crate::scheduler::quota::{get_available_slots, QuotaError};

/// Backoff schedule by attempt count; index = attempt count.
/// Each entry is the minimum minutes since last_dispatch_attempt before we
/// retry.
pub const DISPATCH_BACKOFF_MINUTES: [i64; 7] = [0, 1, 5, 15, 30, 60, 120];
pub const MAX_DISPATCH_BACKOFF_MINUTES: i64 = 240;

/// Reserve the local agent's admission safety buffer
/// (VRAM_SAFETY_BUFFER_GB = 8 in providers/local_agent.py) so we don't
/// yield a job the agent then REFUSES at admission (it rejects when
/// projected_used > total - buffer). Over-committing on raw broadcast
/// free_vram stranded jobs: yielded to the local agent but rejected by it,
/// AND skipped by cloud dispatch because they were yielded. Confirmed live
/// 2026-06-01: a 16GB job yielded to local-ubuntu-server (75/98 GB used,
/// ~22 free) sat unclaimed forever (22 - 8 = 14 < 16). Reserving the
/// buffer routes such jobs to cloud.
pub const LOCAL_ADMISSION_BUFFER_GB: i64 = 8;

/// Scheduler-layer error.
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// Quota read failures (live cloud quotas + overlay).
    #[error(transparent)]
    Quota(#[from] QuotaError),
    /// Queue storage failures.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Provider create/list failures.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// A `${KEY}` the bundled startup templates reference but that no
    /// producer filled. Left unsubstituted the placeholder reaches the VM
    /// verbatim and `set -u` aborts the boot before the agent starts, so
    /// dispatch refuses to create the instance rather than pay for a VM
    /// that can never claim a job.
    #[error(
        "startup-script placeholder ${{{key}}} was never substituted; supply it from \
         scheduler::dispatch::agent::deployment_substitutions or the coordinator secrets"
    )]
    UnresolvedPlaceholder {
        /// Placeholder name only, never its value — secrets stay unlogged.
        key: String,
    },
    /// A template omitted an export owned by the dispatcher. Checking the
    /// source contract before substitution prevents an apparently successful
    /// render from silently dropping deployment state.
    #[error("agent startup template for {provider} omits required ${{{key}}} export")]
    MissingStartupExport { provider: String, key: String },
    /// A required immutable boot coordinate is absent. This must fail before
    /// the provider API is called, not after a billable machine starts.
    #[error(
        "agent startup setting {key} is empty; configure {env} (config key {config_key}) \
         with the immutable runtime artifact before dispatch"
    )]
    MissingStartupSetting {
        key: String,
        env: &'static str,
        config_key: &'static str,
    },
    #[error(
        "agent startup setting {key} is invalid: {reason}; fix {env} \
         (config key {config_key}) before dispatch"
    )]
    InvalidStartupSetting {
        key: String,
        env: &'static str,
        config_key: &'static str,
        reason: &'static str,
    },
}

/// Python `_log`.
pub(crate) fn log(msg: &str) {
    eprintln!("[scheduler] {msg}");
}

/// Python dict repr for `BTreeMap<String, i64>` (`{'a': 1, 'b': 2}`), so
/// stderr logs read exactly like the Python Cloud Function's.
pub(crate) fn py_dict_i64(map: &BTreeMap<String, i64>) -> String {
    let inner: Vec<String> = map.iter().map(|(k, v)| format!("'{k}': {v}")).collect();
    format!("{{{}}}", inner.join(", "))
}

/// Python dict repr for insertion-ordered `(String, i64)` pairs
/// (consumers_by_free_vram order).
pub(crate) fn py_pairs_i64(pairs: &[(String, i64)]) -> String {
    let inner: Vec<String> = pairs.iter().map(|(k, v)| format!("'{k}': {v}")).collect();
    format!("{{{}}}", inner.join(", "))
}

/// Return $/hour for one accelerator of this type at given pricing model.
/// Python `_accel_hourly_rate`.
pub fn accel_hourly_rate(accel_type: &str, preemptible: bool) -> f64 {
    let base = crate::catalog::GPU_HOURLY_RATE_USD
        .get(accel_type)
        .copied()
        .unwrap_or(0.0);
    if !preemptible {
        return base;
    }
    base * crate::catalog::SPOT_DISCOUNT
        .get(accel_type)
        .copied()
        .unwrap_or(0.5)
}

/// True if this job is past its dispatch-backoff window.
/// Python `_backoff_due`.
pub fn backoff_due(job: &Job, now_utc: DateTime<Utc>) -> bool {
    let attempts = job.dispatch_attempts;
    if attempts <= 0 {
        return true;
    }
    let idx = (attempts as usize).min(DISPATCH_BACKOFF_MINUTES.len() - 1);
    let wait_minutes = DISPATCH_BACKOFF_MINUTES[idx].min(MAX_DISPATCH_BACKOFF_MINUTES);
    let Some(last) = &job.last_dispatch_attempt else {
        return true;
    };
    if last.is_empty() {
        return true;
    }
    // Python `datetime.fromisoformat(last.replace("Z", "+00:00"))`.
    let Ok(last_dt) = DateTime::parse_from_rfc3339(&last.replace('Z', "+00:00")) else {
        return true;
    };
    now_utc - last_dt.with_timezone(&Utc) >= Duration::minutes(wait_minutes)
}

/// Autoscale dispatch cap with queue depth. Python `_dynamic_per_tick_cap`.
///
/// Defaults to MAX_SCHEDULE_PER_TICK (4) for shallow queues, scales up for
/// larger bursts so a 723-job batch doesn't drip-feed at 4-per-tick. Upper
/// bound aligned with the multi-region preemptible quota envelope (5
/// regions x ~36 spot GPUs = ~180 ceiling).
pub fn dynamic_per_tick_cap(queue_depth: i64) -> i64 {
    let base = config::MAX_SCHEDULE_PER_TICK;
    if queue_depth <= base * 2 {
        return base;
    }
    // cap=25 fits 60s tick budget
    (base + (queue_depth - base * 2) / 4 + 4).min(25)
}

/// The metadata-only prefilter + priority-desc/FIFO ordering half of
/// Python `schedule_queued_jobs`, split out for tests. Returns the ordered
/// candidate job ids (already capped to `window_budget`) and the count of
/// jobs skipped for sitting on a 0-quota accelerator.
///
/// Metadata-only prefilter (NO body downloads): keep only jobs whose
/// accelerator has available quota this tick, so a backlog of
/// UNDISPATCHABLE jobs cannot saturate the per-tick window and starve
/// dispatchable work. Confirmed live 2026-06-01: 435 jobs sized to
/// nvidia-tesla-k80 (0 fleet k80 quota) filled the 200-job FIFO window
/// every tick -> the only bucket formed was k80 -> "Skip: 0 quota" ->
/// scheduled 0 for the WHOLE fleet, including brand-new t4/l4 jobs queued
/// behind the stuck backlog. write_job stamps gpu_mem_gb, gpu_type, and
/// priority into blob metadata, so this filters + orders the whole queue
/// cheaply and we read only the surviving window's bodies.
/// The stuck backlog stays queued and untouched — it just stops blocking.

fn prefilter_candidates_with_routing(
    blobs: &[crate::queue::BlobInfo],
    available: &BTreeMap<String, i64>,
    provider_name: &str,
    window_budget: usize,
    require_provider_pin: bool,
) -> (Vec<String>, usize) {
    let in_quota: BTreeSet<&str> = available
        .iter()
        .filter(|(_, available)| **available > i64::default())
        .map(|(accelerator, _)| accelerator.as_str())
        .collect();
    let mut cand: Vec<(i64, i64, String)> = Vec::new();
    let mut skipped_no_quota = usize::default();
    for info in blobs {
        if !info.name.ends_with(".json") {
            continue;
        }
        let meta = &info.metadata;
        if require_provider_pin
            && (meta.get("pin_to_provider").map(String::as_str) != Some("true")
                || meta.get("provider").map(String::as_str) != Some(provider_name))
        {
            continue;
        }
        let gm: i64 = meta
            .get("gpu_mem_gb")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let explicit_accel = meta.get("gpu_type").map(|value| value.trim()).unwrap_or("");
        let derived = if gm > i64::default() {
            let (_, accelerator) = config::lookup_instance_type(provider_name, gm);
            accelerator
        } else {
            ""
        };
        let accel_for_filter = if explicit_accel.is_empty() {
            derived
        } else {
            explicit_accel
        };
        if !accel_for_filter.is_empty() && !in_quota.contains(accel_for_filter) {
            skipped_no_quota += true as usize;
            continue;
        }
        let prio: i64 = meta
            .get("priority")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default();
        let ts = info
            .updated
            .map(|updated| updated.timestamp())
            .unwrap_or_default();
        let jid = info
            .name
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".json")
            .to_string();
        cand.push((-prio, ts, jid));
    }
    cand.sort_by_key(|left| (left.0, left.1));
    cand.truncate(window_budget);
    (
        cand.into_iter().map(|(_, _, job_id)| job_id).collect(),
        skipped_no_quota,
    )
}

/// The cost-optimal local-pack knapsack half of Python
/// `schedule_queued_jobs`, split out for tests. Returns job_id ->
/// consumer_id yields.
///
/// COST-OPTIMAL LOCAL PACK: knapsack over queued jobs by
/// $-saved-per-GB-of-local-VRAM, weighted by per-job wall-time so the
/// score reflects total dollars-saved-per-GB on this specific job (not
/// per-hour-of-running). Wall-time comes from the median of past
/// completed jobs of the same (model, gpu_type); when that bucket is
/// empty, a model-size heuristic is used. Best-fit-decreasing packing.
pub(crate) fn local_pack(
    queued: &[Job],
    local_vram_pool: &[(String, i64)],
    wt_table: &BTreeMap<(String, String), f64>,
    now_utc: DateTime<Utc>,
) -> HashMap<String, String> {
    let mut yield_targets: HashMap<String, String> = HashMap::new();
    if local_vram_pool.is_empty() {
        return yield_targets;
    }
    let mut scored: Vec<(f64, i64, &Job)> = Vec::new();
    for j in queued {
        let need = j.gpu_mem_gb;
        if need <= 0 || j.pin_to_provider {
            continue;
        }
        if !backoff_due(j, now_utc) {
            continue;
        }
        let rate = accel_hourly_rate(&j.gpu_type, j.preemptible);
        if rate <= 0.0 {
            continue;
        }
        let wall_s = cost::estimate_wall_time(&j.command, &j.gpu_type, need, wt_table);
        let score = (wall_s / 3600.0) * rate / need as f64; // $-saved per GB on this job
        scored.push((score, need, j));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    // (consumer_id, free-after-admission-buffer) in the pool's original
    // order (consumers_by_free_vram sorts desc); best-fit picks the
    // strictly-largest free entry so iteration order breaks ties exactly
    // like the Python dict scan.
    let mut local_remaining: Vec<(String, i64)> = local_vram_pool
        .iter()
        .map(|(cid, v)| (cid.clone(), (v - LOCAL_ADMISSION_BUFFER_GB).max(0)))
        .collect();
    for (_, need, j) in &scored {
        let mut best: Option<usize> = None;
        for (idx, (_, free_gb)) in local_remaining.iter().enumerate() {
            if *free_gb >= *need && best.is_none_or(|b| *free_gb > local_remaining[b].1) {
                best = Some(idx);
            }
        }
        let Some(best_idx) = best else { continue };
        yield_targets.insert(j.job_id.clone(), local_remaining[best_idx].0.clone());
        local_remaining[best_idx].1 -= need;
    }
    if !yield_targets.is_empty() {
        log(&format!(
            "Cost-optimal local pack: {} jobs yielded; remaining_vram={}",
            yield_targets.len(),
            py_pairs_i64(&local_remaining)
        ));
    }
    yield_targets
}

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

    let available = get_available_slots(store, provider, provider_name).await?;
    log(&format!("Available slots: {}", py_dict_i64(&available)));

    if available.values().all(|v| *v == 0) {
        log("No GPU slots available");
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

    // Read live consumer capacity. Any local agent reporting a free slot
    // for an accelerator is a free-hardware peer we should yield to before
    // paying for a fresh GCE VM. We track yields by accel so a job we
    // yielded in this tick doesn't burn the local agent's capacity in our
    // internal book before it actually claims.
    let consumer_caps = capacity::read_consumer_capacity(store).await?;
    let local_provider = [crate::capabilities::ProviderId::Local.as_str()];
    let local_free = capacity::total_free_by_accel(&consumer_caps, Some(local_provider.as_slice()));
    let local_vram_pool =
        capacity::consumers_by_free_vram(&consumer_caps, Some(local_provider.as_slice()));
    if !local_free.is_empty() {
        log(&format!(
            "Live local-agent slots: {}",
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

