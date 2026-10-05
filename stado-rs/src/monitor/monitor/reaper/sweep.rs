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
use super::super::{dedup_preserve_order, log, py_str_list, MonitorError};
use super::completions::instance_refs_with_completions;
use super::measured::KindDurations;
use super::race::safety_is_real_race;

/// Delete RUNNING VMs whose `wc agent` has stopped doing useful work.
///
/// The agent's main loop publishes a JSON row to
/// gs://<bucket>/capacity/<kind>-<hostname>.json stating when it will publish
/// again. read_consumer_capacity keeps only rows still within that promise.
/// Three reap conditions, none of them a window of anyone's choosing:
///   A (dead agent): no live row, and the VM is older than any VM of its
///     kind has ever taken to publish its first row (measured here).
///   B (never worked): a live row, no completion and no running job, and the
///     VM is older than any VM of its kind has ever taken to finish its
///     first job (measured here).
///   C (wedged): a live row that refuses work with no free VRAM while no job
///     on the VM is alive by its own promise.
/// Every branch defers while any job on the VM is alive by its worker's
/// promise or wrote a pulse or checkpoint after it. With no measurement for
/// a kind, A and B keep the VM and say so.
pub async fn reap_dead_agents(
    store: &JobStorage,
    provider: &dyn Provider,
    kind: &str,
) -> Result<i64, MonitorError> {
    let live = read_consumer_capacity(store).await?; // consumer_id -> payload, promise holds
    let refs = provider.list_running_instance_refs_with_age().await?;
    let mut deleted: i64 = 0;
    let completed_refs = if refs.is_empty() {
        HashSet::new()
    } else {
        instance_refs_with_completions(store, kind).await?
    };
    let mut active_refs: HashSet<String> = HashSet::new();
    for job in store.list_jobs("running", 0).await? {
        if let Some(r) = job.instance_ref.filter(|r| !r.is_empty()) {
            active_refs.insert(r);
        }
    }
    let mut durations = KindDurations::load(store, kind).await?;
    let before = durations.clone();
    let running_names: HashSet<String> = refs
        .iter()
        .map(|(r, _)| r.split('@').next().unwrap_or("").to_string())
        .collect();
    for (instance_ref_full, age_seconds) in &refs {
        let name = instance_ref_full.split('@').next().unwrap_or("");
        if live.contains_key(&format!("{kind}-{name}")) {
            durations.saw_live(name, *age_seconds);
        }
        if completed_refs.contains(&format!("local@{name}")) {
            durations.saw_work(name, *age_seconds);
        }
    }
    durations.keep_only(&running_names);
    if durations != before {
        durations.save(store, kind).await?;
    }
    let ref_to_jids = hg::build_ref_to_jids(store).await?;
    let now = Utc::now();
    for (instance_ref_full, age_seconds) in refs {
        let name = instance_ref_full.split('@').next().unwrap_or("");
        let consumer_id = format!("{kind}-{name}");
        let instance_ref = format!("local@{name}");
        let mut jids = ref_to_jids
            .get(&instance_ref_full)
            .cloned()
            .unwrap_or_default();
        jids.extend(ref_to_jids.get(&instance_ref).cloned().unwrap_or_default());
        if hg::any_job_alive(store, &jids, now).await {
            continue;
        }
        let safety = hg::fresh_jids_pointing_to_ref(store, &instance_ref).await;
        let (reason, requeue_reason) = if !live.contains_key(&consumer_id) {
            // Branch A (dead agent).
            let Some(boot) = durations.boot_seconds else {
                log(&format!(
                    "keep {instance_ref_full}: it has published no capacity row \
                     (age={age_seconds:.0}s) and no {kind} VM has been seen to boot yet, \
                     so nothing says how long a boot takes"
                ));
                continue;
            };
            if age_seconds <= boot {
                continue;
            }
            (
                format!(
                    "dead agent: no live capacity row at age={age_seconds:.0}s, longer than any \
                     {kind} VM has taken to boot ({boot:.0}s)"
                ),
                format!("VM reaped (dead agent, age={age_seconds:.0}s)"),
            )
        } else if !completed_refs.contains(&instance_ref) && !active_refs.contains(&instance_ref) {
            // Branch B (never worked).
            let Some(first_work) = durations.first_work_seconds else {
                continue;
            };
            if age_seconds <= first_work {
                continue;
            }
            (
                format!(
                    "never worked: publishing with 0 completions and no running job at \
                     age={age_seconds:.0}s, longer than any {kind} VM has taken to finish its \
                     first job ({first_work:.0}s)"
                ),
                "VM reaped (never-worked)".to_string(),
            )
        } else {
            // Branch C (wedged).
            let payload = live.get(&consumer_id).unwrap_or(&Value::Null);
            let free_vram_gb = payload
                .get("free_vram_gb")
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
                .unwrap_or(0);
            let refusing = payload.get("accepting_jobs").and_then(Value::as_bool) == Some(false);
            if !(free_vram_gb <= 0 && refusing) {
                continue;
            }
            (
                "wedged: publishing but not accepting jobs with free_vram_gb<=0, and no job on \
                 it alive by its worker's promise"
                    .to_string(),
                "VM reaped (wedged agent)".to_string(),
            )
        };
        if !safety.is_empty() && safety_is_real_race(store, &safety).await {
            log(&format!(
                "defer reap of {instance_ref_full}: running/ {} is alive by its promise",
                py_str_list(&safety)
            ));
            continue;
        }
        provider.delete_instance(&instance_ref_full).await?;
        log(&format!("reaped VM {instance_ref_full} ({reason})"));
        deleted += 1;
        let deduped = dedup_preserve_order([jids, safety].concat());
        requeue_jids_after_reap(store, &deduped, &requeue_reason).await?;
    }
    if deleted > 0 {
        log(&format!("reap_dead_agents: deleted {deleted} VM(s)"));
    }
    Ok(deleted)
}
