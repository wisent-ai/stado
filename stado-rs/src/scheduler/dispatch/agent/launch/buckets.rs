//! Grouping eligible queued work into the (accel, machine_type) buckets a
//! tick dispatches against.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::config;
use crate::models::Job;
use crate::queue::JobStorage;
use crate::scheduler::scheduler::{accel_hourly_rate, backoff_due, SchedulerError};
use crate::sizing::Sizing;

/// The bucketing half of Python `dispatch_agent_vms`, split out for
/// tests. Bucket key is (accel, mt); bucket order is first-seen
/// (Python dict insertion order), which decides which buckets win the
/// per-tick cap when the queue is deep.
///
/// Bucket key is (accel, mt). Default: derive from current GPU_SIZING
/// via lookup_instance_type — protects against stale job-level machine
/// specs (e.g. a2-highgpu-2g + nvidia-tesla-a100 for 60GB jobs that GCP
/// rejects with 'Invalid accelerator specs for accelerator optimized
/// instances'). Override: caller-pinned job.machine_type wins so users
/// who need a specific host (g2-standard-8 for 32 GB RAM on an L4 job)
/// don't get silently downgraded back to the default-tier g2-standard-4
/// (16 GB RAM, repeated host-OOM source for diffusion training).
pub(crate) async fn bucket_jobs(
    queued: &[Job],
    yield_targets: &HashMap<String, String>,
    provider_name: &str,
    sizing: &Sizing,
    store: &JobStorage,
    now_utc: DateTime<Utc>,
) -> Result<Vec<((String, String), Vec<Job>)>, SchedulerError> {
    let mut buckets: Vec<((String, String), Vec<Job>)> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for j in queued {
        if j.pin_to_provider && j.provider != provider_name {
            continue;
        }
        if yield_targets.contains_key(&j.job_id) {
            continue;
        }
        if !backoff_due(j, now_utc) {
            continue;
        }
        let mut gpu_mem = j.gpu_mem_gb;
        if gpu_mem <= 0 {
            // Unmeasured on the queue blob — normalize_queue_sizing forces
            // gpu_mem_gb=0 whenever observed_vram_gb(model) is None at
            // sizing time (sizing/__init__.py docstring). Previously this
            // branch was a hard `continue`, which combined with the
            // always-write-0 behaviour to lock the entire
            // unmeasured-model queue out of the autoscaler (199
            // gpt-oss-20b jobs stuck at gpu_mem_gb=0 observed live
            // 2026-05-20). Recover by:
            //   1. Re-checking observed_vram_gb (a sibling job of the
            //      same model may have just completed and populated the
            //      map),
            //   2. Using smallest_live_vram() instead — the documented
            //      start size for unmeasured models (see
            //      escalate_on_oom). If the job overflows that tier,
            //      escalate_on_oom climbs to next_live_vram on requeue.
            // If neither yields a number, the fleet has no live GPU
            // broadcasting at all and the job is genuinely unschedulable
            // this tick; defer to the next.
            let model = crate::sizing::model_of(&j.command);
            let peak = if model.is_empty() {
                None
            } else {
                sizing.observed_vram_gb(store, &model).await?
            };
            if let Some(peak) = peak {
                if peak > 0 {
                    gpu_mem = peak;
                }
            }
            if gpu_mem <= 0 {
                let Some(live_small) = sizing.smallest_live_vram(store).await? else {
                    continue;
                };
                gpu_mem = live_small;
            }
        }
        let (default_mt, default_accel) = config::lookup_instance_type(provider_name, gpu_mem);
        if default_accel.is_empty() || default_mt.is_empty() {
            continue;
        }
        // Caller-pinned overrides — use the catalog if either is
        // empty.
        //
        // A pin only wins for the provider whose naming it uses. Every CPU job
        // carries `e2-standard-8` from `queue::submit`, a GCE name; honouring
        // it while dispatching to Azure produced `hardwareProfile.vmSize:
        // e2-standard-8`, which Azure rejects. A pin aimed at another cloud is
        // therefore not a preference to respect, it is a stale artifact of the
        // cloud the job was submitted under.
        let mt = {
            let pinned = j.machine_type.trim();
            let pin_provider = crate::catalog::machine_type_provider(pinned);
            let usable_pin =
                !pinned.is_empty() && pin_provider.is_none_or(|owner| owner == provider_name);
            if usable_pin {
                pinned
            } else {
                default_mt
            }
        };
        let accel = {
            let pinned = j.gpu_type.trim();
            if pinned.is_empty() {
                default_accel
            } else {
                pinned
            }
        };
        let cap = j.max_cost_per_hour_usd;
        if cap > 0.0 && !accel.is_empty() {
            let rate = accel_hourly_rate(accel, j.preemptible);
            if rate > 0.0 && rate > cap {
                continue;
            }
        }
        let key = (accel.to_string(), mt.to_string());
        match index.get(&key) {
            Some(&idx) => buckets[idx].1.push(j.clone()),
            None => {
                index.insert(key.clone(), buckets.len());
                buckets.push((key, vec![j.clone()]));
            }
        }
    }
    Ok(buckets)
}
