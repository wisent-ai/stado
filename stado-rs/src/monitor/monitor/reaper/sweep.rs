//! The fleet sweep itself: one pass over the provider's RUNNING VMs applying
//! the dead-agent, never-worked and wedged conditions, each guarded by the
//! signals that prove a VM is still productive.

use std::collections::HashSet;

use chrono::Utc;
use serde_json::Value;

use crate::monitor::heartbeat_guard as hg;
use crate::providers::Provider;
use crate::queue::capacity::read_consumer_capacity;
use crate::queue::JobStorage;

use super::super::lifecycle::requeue_jids_after_reap;
use super::super::{dedup_preserve_order, elapsed_seconds, log, py_str_list, MonitorError};
use super::completions::instance_refs_with_completions;
use super::race::safety_is_real_race;

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
    //   Branch C (wedged): broadcasting fresh capacity BUT not accepting jobs,
    //     free_vram_gb<=0, last claim/start (diag) stale, and no job on this VM
    //     heartbeating. A hung claimed subprocess pins VRAM forever while the
    //     top-of-loop heartbeat keeps the worker publication fresh, so it is
    //     invisible to Branch A (capacity fresh) and Branch B (historical
    //     completions keep it in completed_refs). Confirmed live 2026-05-17
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
        // Branch C (wedged): fresh publication, explicit admission refusal,
        // no free VRAM, stale claim/start diagnostics, and no fresh job
        // heartbeat. The heartbeat guard protects a healthy long trainer; the
        // diagnostic-age guard protects a worker that is actively claiming.
        let empty_payload = Value::Null;
        let payload = live.get(&consumer_id).unwrap_or(&empty_payload);
        let free_vram_gb = payload
            .get("free_vram_gb")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .unwrap_or(0);
        let refusing_jobs = payload.get("accepting_jobs").and_then(Value::as_bool) == Some(false);
        if free_vram_gb <= 0 && refusing_jobs {
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
                "reaped wedged VM {instance_ref_full} (capacity fresh but not accepting jobs \
                 and free_vram_gb<=0, last claim/start {last_str} stale \
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
