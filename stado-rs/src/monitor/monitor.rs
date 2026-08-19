//! Monitor running jobs: check heartbeat, status, cleanup.
//!
//! Port of `stado/monitor/monitor.py` (`check_running_jobs` +
//! `reap_dead_agents`) and `stado/monitor/reap/helpers.py` (the
//! monitor-internal requeue/completed-ref helpers).
//!
//! Handles four exit conditions for a running job:
//!   COMPLETED         -> finalize success path
//!   FAILED            -> finalize failure path + alert
//!   preempted (Spot)  -> instance is TERMINATED but the Job is otherwise healthy.
//!                       Delete the GCE instance, increment preempt_count, requeue.
//!                       preempt_count is separate from restarts so a Spot-heavy
//!                       job doesn't burn the restart budget on preemptions alone.
//!   instance gone OR
//!   stale heartbeat   -> requeue (counted against restarts).

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::alerts::send_alert;
use super::heartbeat_guard as hg;
use crate::config;
use crate::models::{isoformat_utc, job_state, Job};
use crate::providers::{Provider, ProviderError};
use crate::queue::capacity::read_consumer_capacity;
use crate::queue::{JobStorage, StorageError};

/// Monitor-layer error. Python lets storage/provider exceptions propagate
/// out of the per-tick functions; both source layers map onto one error so
/// `?` does the same.
#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    /// Storage (queue/blob) failures.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Provider (GCE/AWS/Azure API) failures.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Python `_log`: stderr with the [monitor] prefix.
pub(crate) fn log(msg: &str) {
    eprintln!("[monitor] {msg}");
}

/// `(now - then)` in float seconds — Python `timedelta.total_seconds()`.
fn elapsed_seconds(now: DateTime<Utc>, then: DateTime<Utc>) -> f64 {
    (now - then).num_milliseconds() as f64 / 1000.0
}

/// Python repr of a list of strings (`['a', 'b']`) for log-line parity
/// with the Cloud Function logs operators grep.
fn py_str_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", inner.join(", "))
}

/// Python `list(dict.fromkeys(items))`: dedup preserving first-seen order.
fn dedup_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert(i.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// reap/helpers.py ports (monitor-internal)
// ---------------------------------------------------------------------------

/// Best-effort delete of a GCE VM named `hostname`.
///
/// The requeue paths in check_running_jobs (orphan + VM-gone) previously
/// moved running -> queue without calling provider.delete_instance, so
/// the prior agent's training subprocess kept running on the
/// supposedly-gone VM and producing duplicate writes against the same
/// gs://wisent-compute/ckpts/<run>/ path. Confirmed live 2026-05-18 for
/// job 724084db: 4 concurrent trainers (workstation + 3 GCP VMs) all
/// racing on the same ckpt prefix because each transient "VM missing
/// from fleet listing" requeue spawned a new dispatch without
/// terminating the old subprocess.
///
/// Looks up the full <name>@<zone> ref from `vm_cache` (a dict
/// {hostname: full_ref} built by the caller) and falls back to a fresh
/// list-and-search on cache miss (handles the exact transient-miss case
/// that caused the ghost-trainer bug). Never raises; returns True iff
/// a delete call returned cleanly. Idempotent and safe on a VM that is
/// truly gone (returns False).
async fn safe_delete_vm_by_hostname(
    provider: &dyn Provider,
    hostname: &str,
    vm_cache: &BTreeMap<String, String>,
) -> bool {
    let mut full_ref = vm_cache.get(hostname).cloned();
    if full_ref.is_none() {
        match provider.list_running_instance_refs_with_age().await {
            Ok(refs) => {
                for (r, _age) in refs {
                    if r.split('@').next() == Some(hostname) {
                        full_ref = Some(r);
                        break;
                    }
                }
            }
            Err(e) => {
                log(&format!(
                    "safe_delete: fresh list failed for {hostname}: {e:?}"
                ));
                return false;
            }
        }
    }
    let Some(full_ref) = full_ref else {
        return false;
    };
    match provider.delete_instance(&full_ref).await {
        Ok(()) => {
            log(&format!("safe_delete: killed ghost VM {full_ref}"));
            true
        }
        Err(e) => {
            log(&format!("safe_delete({full_ref}) failed: {e:?}"));
            false
        }
    }
}

/// Move job back to queue or fail if max restarts exceeded.
async fn requeue(store: &JobStorage, job: &mut Job, reason: &str) -> Result<(), MonitorError> {
    job.restarts += 1;
    if job.restarts > job.max_restarts {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(Utc::now()));
        job.error = Some(format!("Exceeded {} restarts ({reason})", job.max_restarts));
        // Python parity: NO cleanup_status on the restart-cap path.
        store.move_job(job, "running", "failed").await?;
        log(&format!("{}: FAILED (restart cap, {reason})", job.job_id));
        return Ok(());
    }

    job.state = job_state::QUEUED.to_string();
    job.instance_ref = None;
    job.started_at = None;
    job.last_restart = Some(isoformat_utc(Utc::now()));
    store.move_job(job, "running", "queue").await?;
    store.cleanup_status(&job.job_id).await?;
    log(&format!(
        "{}: requeued ({reason}, restart {})",
        job.job_id, job.restarts
    ));
    Ok(())
}

/// Return set of instance_ref strings appearing in completed/.
///
/// DEVIATION from Python: `_instance_refs_with_completions` keeps the set
/// in a process-global 300s-TTL cache because the Cloud Function is
/// short-lived and the completed/ scan (~13.5k blobs, ~75s) blew the tick
/// budget. Here the cache would have to live in a global; a per-tick
/// rebuild is correct-but-slower and acceptable for the long-running
/// daemon. The `needs_completions_scan` short-circuit in reap_dead_agents
/// (only scan when some VM crossed IDLE_GRACE_SECONDS) is the real guard
/// and is preserved.
async fn instance_refs_with_completions(
    store: &JobStorage,
    kind: &str,
) -> Result<HashSet<String>, MonitorError> {
    // Python keeps `kind` in the signature (unused in the body there too).
    let _ = kind;
    let mut refs = HashSet::new();
    for job in store.list_jobs("completed", 0).await? {
        if let Some(r) = job.instance_ref.filter(|r| !r.is_empty()) {
            refs.insert(r);
        }
    }
    Ok(refs)
}

/// Decide whether a freshly re-listed running/ jid set
/// (fresh_jids_pointing_to_ref) is a GENUINE active_refs race that must
/// defer a reap, vs a set of confirmed ORPHANS safe to reap+requeue.
///
/// fresh_jids_pointing_to_ref's "fresh" means "re-listed at call time"
/// (beats the cached-listing race), NOT "the job is alive" — it returns
/// every running/ blob pointing at the ref with zero liveness check. On
/// a CONFIRMED-dead agent (reaper Branch A: consumer_id absent from live
/// capacity) that made the guard defer on mere blob existence forever:
/// 0db3438b/6a0fceba sat ~3h on agents that were gone (no capacity
/// broadcast, heartbeats ~2.5h stale), never requeued, and the whole
/// gpt-oss-20b queue totally stalled (2026-05-19).
///
/// A real race is only when the job is plausibly alive: the GCS re-list
/// itself failed (fail-safe defer), OR some jid still heartbeats / writes
/// checkpoints fresh, OR some jid started so recently it has not had time
/// to heartbeat yet (boot grace). Otherwise every jid is a stale orphan
/// on a dead VM -> return False so the caller reaps and requeues them.
async fn safety_is_real_race(
    store: &JobStorage,
    jids: &[String],
    hb_threshold: f64,
) -> Result<bool, MonitorError> {
    if jids.is_empty() {
        return Ok(false);
    }
    if jids.iter().any(|j| j == hg::LIST_FAILED_SENTINEL) {
        return Ok(true); // GCS re-list failure: never reap on unknown state
    }
    if hg::any_job_heartbeat_fresh(store, jids, hb_threshold).await
        || hg::any_job_checkpoint_fresh_jids(store, jids, 5400.0).await
    {
        return Ok(true);
    }
    // Python does NOT catch list_jobs errors here — propagate via `?`.
    let running: BTreeMap<String, Job> = store
        .list_jobs("running", 0)
        .await?
        .into_iter()
        .map(|j| (j.job_id.clone(), j))
        .collect();
    let now = Utc::now();
    for jid in jids {
        let Some(job) = running.get(jid) else {
            continue;
        };
        let Some(sa) = job.started_at.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        // Python: except (ValueError, TypeError) -> continue.
        let Some(started) = hg::parse_iso_lenient(sa) else {
            continue;
        };
        if elapsed_seconds(now, started) < 1800.0 {
            return Ok(true); // just dispatched, no heartbeat yet (real race)
        }
    }
    Ok(false)
}

/// Requeue the given jids (looked up fresh in running/) after their VM
/// was reaped.
async fn requeue_jids_after_reap(
    store: &JobStorage,
    jids: &[String],
    reason: &str,
) -> Result<(), MonitorError> {
    if jids.is_empty() {
        return Ok(());
    }
    let running: BTreeMap<String, Job> = store
        .list_jobs("running", 0)
        .await?
        .into_iter()
        .map(|j| (j.job_id.clone(), j))
        .collect();
    for jid in jids {
        let Some(job) = running.get(jid).cloned() else {
            continue;
        };
        let mut job = job;
        requeue(store, &mut job, reason).await?;
    }
    Ok(())
}

/// Move job back to queue, counting preemption separately from restarts.
///
/// Preemptions are an expected part of Spot lifecycle, not a fault. They
/// accumulate in preempt_count; once that exceeds max_preempts_before_ondemand
/// the scheduler dispatches the next attempt on-demand instead.
async fn requeue_preempted(
    store: &JobStorage,
    job: &mut Job,
    reason: &str,
) -> Result<(), MonitorError> {
    job.preempt_count += 1;
    job.state = job_state::QUEUED.to_string();
    job.instance_ref = None;
    job.started_at = None;
    job.last_restart = Some(isoformat_utc(Utc::now()));
    store.move_job(job, "running", "queue").await?;
    store.cleanup_status(&job.job_id).await?;
    log(&format!(
        "{}: requeued ({reason}, preempts={})",
        job.job_id, job.preempt_count
    ));
    Ok(())
}

/// Requeue a job orphaned on a stale non-cloud local@ agent.
///
/// reap_dead_agents only iterates GCP provider VMs, so a local@<host>
/// that is NOT a wisent-agent-* cloud VM (e.g. the ubuntu-server lab
/// box) is never reaped there. In check_running_jobs the agent_live
/// block is skipped once that agent's capacity broadcast goes stale,
/// and the is_cloud_agent_name block does not match, so control fell
/// to a bare `continue` and the job wedged in running/ forever
/// (0db3438b: hb 03:28:42, command_output.log 03:25:46,
/// local-ubuntu-server capacity stale, coordinator logged 'dead host
/// ubuntu-server', never requeued, 2026-05-19). Leave the job alone
/// only if it is demonstrably alive (fresh heartbeat / fresh
/// checkpoint) or its command self-terminates the agent (kill is the
/// success condition); otherwise requeue it. No VM delete — the local
/// host is operator-owned and must not be touched.
async fn requeue_dead_local_host_orphan(
    store: &JobStorage,
    job: &mut Job,
    job_id: &str,
) -> Result<(), MonitorError> {
    if hg::any_job_heartbeat_fresh(store, &[job_id.to_string()], 1800.0).await {
        return Ok(());
    }
    if hg::any_job_checkpoint_fresh(store, job, 5400.0).await {
        return Ok(());
    }
    if hg::finalize_if_self_terminating(store, job, &log).await? {
        return Ok(());
    }
    requeue(
        store,
        job,
        "local agent capacity stale & job heartbeat stale (dead local host orphan)",
    )
    .await
}

// ---------------------------------------------------------------------------
// monitor.py ports
// ---------------------------------------------------------------------------

/// Check all running jobs. Handle completion, failure, preemption, stale.
pub async fn check_running_jobs(
    store: &JobStorage,
    provider: &dyn Provider,
) -> Result<(), MonitorError> {
    let running = store.list_jobs("running", 0).await?;
    log(&format!("Checking {} running jobs", running.len()));

    // Lazily-built per-call caches (Python: _live_consumers_cache /
    // _running_vm_names_cache, built at most once per invocation).
    let mut live_consumers_cache: Option<BTreeMap<String, Value>> = None;
    let mut running_vm_names_cache: Option<BTreeMap<String, String>> = None;

    for mut job in running {
        let job_id = job.job_id.clone();
        let Some(instance_ref) = job.instance_ref.clone().filter(|r| !r.is_empty()) else {
            requeue(store, &mut job, "no instance ref").await?;
            continue;
        };

        let status = store.read_status(&job_id).await?;

        if status.as_deref() == Some("COMPLETED") {
            job.state = job_state::COMPLETED.to_string();
            job.completed_at = Some(isoformat_utc(Utc::now()));
            provider.delete_instance(&instance_ref).await?;
            store.move_job(&job, "running", "completed").await?;
            store.cleanup_status(&job_id).await?;
            log(&format!("{job_id}: COMPLETED"));
        } else if status.as_deref() == Some("FAILED") {
            job.state = job_state::FAILED.to_string();
            job.failed_at = Some(isoformat_utc(Utc::now()));
            provider.delete_instance(&instance_ref).await?;
            store.move_job(&job, "running", "failed").await?;
            store.cleanup_status(&job_id).await?;
            let msg = format!(
                "Job {job_id} FAILED: {}",
                job.command.chars().take(100).collect::<String>()
            );
            // Best-effort: alert failures never block the monitor tick.
            send_alert(config::alerts_topic(), &msg, "").await;
            log(&format!("{job_id}: FAILED"));
        } else {
            // Boot grace: a freshly (re)dispatched job has not yet
            // written its first heartbeat (agent claim -> apt/clone/pip/
            // multi-GB ckpt-pull preamble before slots.py Popen +
            // _write_heartbeat) while the previous run's heartbeat blob
            // is already aged, so the orphan / VM-gone staleness guards
            // below false-positive and requeue a healthy starting job
            // (synchronized 3ef705b2+724084db requeues 16:18/20:00/
            // 21:33/21:57 on RUNNING 0.4.228 VMs). Skip requeue logic
            // until the job has had BOOT_GRACE_SECONDS to heartbeat.
            if let Some(sa) = job.started_at.as_deref().filter(|s| !s.is_empty()) {
                // Python: except (ValueError, TypeError) -> pass (parse
                // errors fall through to the guards below).
                if let Some(started) = hg::parse_iso_lenient(sa) {
                    if elapsed_seconds(Utc::now(), started) < 1800.0 {
                        continue;
                    }
                }
            }
            if let Some(hostname) = instance_ref.strip_prefix("local@") {
                if live_consumers_cache.is_none() {
                    live_consumers_cache = Some(read_consumer_capacity(store).await?);
                }
                let live = live_consumers_cache.as_ref().expect("just built");
                let agent_live = crate::capabilities::get("execution")
                    .into_iter()
                    .flat_map(|capability| capability.variants)
                    .any(|variant| live.contains_key(&format!("{}-{hostname}", variant.id)));
                if agent_live {
                    // Agent up != this old job progresses (restarts
                    // orphan it). Heartbeat is proof; self-terminating
                    // cmds (pkill wc agent) -> kill IS success.
                    if hg::any_job_heartbeat_fresh(store, std::slice::from_ref(&job_id), 1800.0)
                        .await
                    {
                        continue;
                    }
                    if hg::finalize_if_self_terminating(store, &mut job, &log).await? {
                        continue;
                    }
                    if !hg::any_job_checkpoint_fresh(store, &job, 5400.0).await {
                        let cache = running_vm_names_cache.clone().unwrap_or_default();
                        safe_delete_vm_by_hostname(provider, hostname, &cache).await;
                        requeue(
                            store,
                            &mut job,
                            "local agent live but job heartbeat stale (orphan)",
                        )
                        .await?;
                    }
                    continue;
                }
                if hostname.starts_with("wisent-agent-") {
                    if running_vm_names_cache.is_none() {
                        running_vm_names_cache = Some(
                            provider
                                .list_running_instance_refs_with_age()
                                .await?
                                .into_iter()
                                .map(|(r, _age)| (r.split('@').next().unwrap_or("").to_string(), r))
                                .collect(),
                        );
                    }
                    let cache = running_vm_names_cache.as_ref().expect("just built");
                    if !cache.contains_key(hostname) {
                        // fresh job heartbeat = VM+agent+training alive;
                        // aggregated_list missed a transient non-RUNNING
                        // (STAGING/REPAIRING/live-migration) snapshot
                        if hg::any_job_heartbeat_fresh(store, std::slice::from_ref(&job_id), 1800.0)
                            .await
                        {
                            continue;
                        }
                        safe_delete_vm_by_hostname(provider, hostname, cache).await;
                        if job.preemptible {
                            requeue_preempted(store, &mut job, "Spot preempted (cloud agent gone)")
                                .await?;
                        } else {
                            requeue(store, &mut job, "VM gone (cloud agent missing from fleet)")
                                .await?;
                        }
                        continue;
                    }
                }
                requeue_dead_local_host_orphan(store, &mut job, &job_id).await?;
                continue;
            }

            let alive = provider.instance_exists(&instance_ref).await?;
            let lifecycle = provider.instance_lifecycle_state(&instance_ref).await?;

            if !alive && lifecycle.as_deref() == Some("TERMINATED") && job.preemptible {
                requeue_preempted(store, &mut job, "Spot preempted").await?;
                provider.delete_instance(&instance_ref).await?;
            } else if !alive {
                // Python f-string renders a None lifecycle as "None".
                let lifecycle_str = lifecycle.as_deref().unwrap_or("None");
                requeue(
                    store,
                    &mut job,
                    &format!("instance gone (lifecycle={lifecycle_str})"),
                )
                .await?;
                provider.delete_instance(&instance_ref).await?;
            }
        }
    }
    Ok(())
}

/// Delete RUNNING VMs whose `wc agent` has stopped doing useful work.
///
/// The agent's main loop publishes a freshness-stamped JSON to
/// gs://<bucket>/capacity/<kind>-<hostname>.json on every iteration. If the
/// process crashes (OOM, segfault, uncaught exception) the GCE instance keeps
/// running, holding GPU + disk quota with zero work output. read_consumer_capacity
/// filters to broadcasts younger than CAPACITY_STALE_SECONDS. Any RUNNING VM
/// whose corresponding consumer_id is missing from that filtered set has a
/// dead agent and gets deleted here so the dispatcher can spawn a fresh
/// replacement.
pub async fn reap_dead_agents(
    store: &JobStorage,
    provider: &dyn Provider,
    kind: &str,
) -> Result<i64, MonitorError> {
    let live = read_consumer_capacity(store).await?; // consumer_id -> payload, fresh only
    let refs = provider.list_running_instance_refs_with_age().await?;
    let mut deleted: i64 = 0;
    // Three reap conditions, each age/liveness-guarded so a VM still in its
    // startup-script install phase (~10-14 min on provider base images) is not killed
    // before it can work.
    //   Branch A (dead-agent): age > BOOT_GRACE AND no fresh capacity
    //     broadcast. Covers crashed agents + startup-script failures.
    //   Branch B (never-worked): age > IDLE_GRACE AND broadcasting AND zero
    //     completions in completed/ for this instance_ref.
    //   Branch C (wedged): broadcasting fresh capacity BUT free_vram_gb<=0
    //     AND free_slots={} AND last claim/start (diag) stale AND no job on
    //     this VM heartbeating. A hung claimed subprocess pins VRAM forever
    //     (advance_slot only retires on proc.poll() != None), free_vram_gb=0
    //     short-circuits the agent loop before the diag write, and the
    //     top-of-loop re-publish keeps capacity fresh — so the VM is invisible
    //     to Branch A (capacity fresh) AND Branch B (historical completions
    //     keep it in completed_refs). Confirmed live 2026-05-17
    //     (gcp-wisent-agent-80gb-1778921111-0: free_vram_gb=0, last_started_at
    //     frozen 2026-05-16T09:17:32, 127 gpt-oss-20b jobs dead-pinned hours).
    // BOOT/IDLE 1800s: 900s reaped real 14m boots (3ef705b2/931b865e/f3fd41fb
    // ricocheting dispatch<->reap, confirmed 2026-05-15 02:24Z).
    const BOOT_GRACE_SECONDS: f64 = 1800.0;
    const IDLE_GRACE_SECONDS: f64 = 1800.0; // half-window grace for first completion
                                            // Build the completed-refs set ONLY if any VM is old enough to need it.
                                            // Iterating completed/ at fleet scale (~11k blobs) blows the 60s tick
                                            // budget every time, returning 504 and pausing Cloud Scheduler. Cheap
                                            // short-circuit: if no VM has crossed IDLE_GRACE_SECONDS, branch B
                                            // cannot fire anyway.
    let needs_completions_scan = refs
        .iter()
        .any(|(_, age_seconds)| *age_seconds > IDLE_GRACE_SECONDS);
    let completed_refs = if needs_completions_scan {
        instance_refs_with_completions(store, kind).await?
    } else {
        HashSet::new()
    };
    // ALSO build the set of VMs that currently have a job in running/. A VM
    // mid-extraction on its FIRST big job (e.g. gpt-oss-20b 80GB shards)
    // legitimately exceeds IDLE_GRACE_SECONDS=1800 before producing its
    // first completion. Without this check, the never-worked reaper kills
    // healthy VMs and the parent jobs ricochet through restart cycles.
    // Confirmed live on 2026-05-07: reaper killed 23+ working VMs in one
    // hour, triggering the "never-worked reap (>5 in 1h)" alert email
    // storm.
    let mut active_refs: HashSet<String> = HashSet::new();
    if needs_completions_scan {
        for job in store.list_jobs("running", 0).await? {
            if let Some(r) = job.instance_ref.filter(|r| !r.is_empty()) {
                active_refs.insert(r);
            }
        }
    }
    // Second signal: per-job heartbeat. Defers the reap when the agent's
    // capacity blob is stale BUT a running job assigned to its VM still
    // has a fresh heartbeat — agent is alive, just starved on its
    // broadcast tick by a training subprocess. Without this guard the
    // reaper destroys productive VMs (Llama-1B 5k run was reaped 3 times
    // mid-training on 2026-05-12 because rollout steps exceeded
    // CAPACITY_STALE_SECONDS).
    let ref_to_jids = hg::build_ref_to_jids(store).await?;
    const HB_THRESHOLD: f64 = 1800.0;
    for (instance_ref_full, age_seconds) in refs {
        let name = instance_ref_full.split('@').next().unwrap_or("");
        let consumer_id = format!("{kind}-{name}");
        let instance_ref = format!("local@{name}");
        if !live.contains_key(&consumer_id) {
            // Branch A (dead-agent).
            if age_seconds < BOOT_GRACE_SECONDS {
                continue; // still installing, give it time
            }
            let mut jids = ref_to_jids
                .get(&instance_ref_full)
                .cloned()
                .unwrap_or_default();
            jids.extend(ref_to_jids.get(&instance_ref).cloned().unwrap_or_default());
            if hg::any_job_heartbeat_fresh(store, &jids, HB_THRESHOLD).await
                || hg::any_job_checkpoint_fresh_jids(store, &jids, 5400.0).await
            {
                log(&format!(
                    "defer reap of {instance_ref_full}: capacity stale \
                     (age={age_seconds:.0}s) but job heartbeat fresh for {}",
                    py_str_list(&jids)
                ));
                continue;
            }
            let safety = hg::fresh_jids_pointing_to_ref(store, &instance_ref).await;
            if !safety.is_empty() && safety_is_real_race(store, &safety, HB_THRESHOLD).await? {
                log(&format!(
                    "defer dead-agent reap of {instance_ref_full}: live/starting running/ {}",
                    py_str_list(&safety)
                ));
                continue;
            }
            provider.delete_instance(&instance_ref_full).await?;
            log(&format!(
                "reaped dead-agent VM {instance_ref_full} (no fresh capacity broadcast, \
                 age={age_seconds:.0}s > boot grace {BOOT_GRACE_SECONDS}s, \
                 no fresh job heartbeat either)"
            ));
            deleted += 1;
            let deduped = dedup_preserve_order([jids, safety].concat());
            requeue_jids_after_reap(
                store,
                &deduped,
                &format!("VM reaped (dead agent, age={age_seconds:.0}s)"),
            )
            .await?;
            continue;
        }
        if age_seconds > IDLE_GRACE_SECONDS
            && !completed_refs.contains(&instance_ref)
            && !active_refs.contains(&instance_ref)
        {
            // Branch B (never-worked). Branch A defers on a fresh job
            // heartbeat; Branch B must too. A long training run never
            // appears in completed/ and is protected only by the
            // race-prone active_refs set, so a working VM (Llama 3ef705b2
            // at step ~3533, heartbeat fresh via the 0.4.224 daemon
            // thread) was reaped here as "never-worked" at
            // 2026-05-15T23:14:01 (restart 8). A fresh job heartbeat is
            // proof the VM is productive — never reap.
            let mut jids_b = ref_to_jids
                .get(&instance_ref_full)
                .cloned()
                .unwrap_or_default();
            jids_b.extend(ref_to_jids.get(&instance_ref).cloned().unwrap_or_default());
            if hg::any_job_heartbeat_fresh(store, &jids_b, HB_THRESHOLD).await
                || hg::any_job_checkpoint_fresh_jids(store, &jids_b, 5400.0).await
            {
                log(&format!(
                    "defer never-worked reap of {instance_ref_full}: job heartbeat fresh for {}",
                    py_str_list(&jids_b)
                ));
                continue;
            }
            let safety = hg::fresh_jids_pointing_to_ref(store, &instance_ref).await;
            if !safety.is_empty() {
                log(&format!(
                    "defer never-worked reap of {instance_ref_full}: fresh running/ found {} \
                     (active_refs race; root cause of 724084db restart 16 wedge \
                     2026-05-17T21:26:07)",
                    py_str_list(&safety)
                ));
                continue;
            }
            provider.delete_instance(&instance_ref_full).await?;
            log(&format!(
                "reaped never-worked VM {instance_ref_full} (broadcasting but 0 completions \
                 AND no active running job in age={age_seconds:.0}s, \
                 > grace {IDLE_GRACE_SECONDS}s)"
            ));
            deleted += 1;
            requeue_jids_after_reap(store, &jids_b, "VM reaped (never-worked)").await?;
            continue;
        }
        // Branch C (wedged): fresh capacity but structurally stuck. ALL of:
        // free_vram_gb<=0 AND empty free_slots, diag last_started_at /
        // last_claim_attempt_at older than HB_THRESHOLD, no fresh heartbeat
        // for any job on this VM. The heartbeat guard protects a healthy long
        // trainer whose broadcast tick is merely starved; the diag-stale
        // guard protects an agent that is actively claiming.
        let empty_payload = Value::Null;
        let payload = live.get(&consumer_id).unwrap_or(&empty_payload);
        let free_vram_gb = payload
            .get("free_vram_gb")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0);
        let free_slots_empty = payload
            .get("free_slots")
            .and_then(Value::as_object)
            .is_none_or(|o| o.is_empty());
        if free_vram_gb <= 0 && free_slots_empty {
            let diag = payload.get("diag").and_then(Value::as_object);
            let last = diag
                .and_then(|d| d.get("last_started_at"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    diag.and_then(|d| d.get("last_claim_attempt_at"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                });
            let mut stale = false;
            if let Some(last_str) = last {
                // Python: str(_last).replace("Z", "+00:00") then
                // fromisoformat; unparseable/missing -> NOT stale -> skip.
                if let Some(dt) = hg::parse_iso_lenient(&last_str.replace('Z', "+00:00")) {
                    stale = elapsed_seconds(Utc::now(), dt) > HB_THRESHOLD;
                }
            }
            if !stale {
                continue;
            }
            let last_str = last.unwrap_or("");
            let mut jids_c = ref_to_jids
                .get(&instance_ref_full)
                .cloned()
                .unwrap_or_default();
            jids_c.extend(ref_to_jids.get(&instance_ref).cloned().unwrap_or_default());
            if hg::any_job_heartbeat_fresh(store, &jids_c, HB_THRESHOLD).await
                || hg::any_job_checkpoint_fresh_jids(store, &jids_c, 5400.0).await
            {
                log(&format!(
                    "defer wedged reap of {instance_ref_full}: job heartbeat fresh for {}",
                    py_str_list(&jids_c)
                ));
                continue;
            }
            let safety = hg::fresh_jids_pointing_to_ref(store, &instance_ref).await;
            if !safety.is_empty() {
                log(&format!(
                    "defer wedged reap of {instance_ref_full}: fresh running/ found {} \
                     (active_refs race)",
                    py_str_list(&safety)
                ));
                continue;
            }
            provider.delete_instance(&instance_ref_full).await?;
            log(&format!(
                "reaped wedged VM {instance_ref_full} (capacity fresh but free_vram_gb<=0 \
                 & no free_slots, last claim/start {last_str} stale \
                 > {HB_THRESHOLD}s, no fresh job heartbeat)"
            ));
            deleted += 1;
            requeue_jids_after_reap(store, &jids_c, "VM reaped (wedged agent)").await?;
            continue;
        }
    }
    if deleted > 0 {
        log(&format!("reap_dead_agents: deleted {deleted} VM(s)"));
    }
    Ok(deleted)
}
