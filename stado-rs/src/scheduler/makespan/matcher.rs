//! The per-job placement decision: VRAM-concurrency arithmetic on one
//! worker's projected slots, accelerator compatibility, and the earliest-
//! finish pick across every eligible worker. Split out of `makespan/mod.rs`;
//! the worker projection it reads is built in `agents`.

use std::collections::BTreeMap;

use crate::catalog::GPU_SIZING;
use crate::models::Job;

use super::agents::AgentInfo;

/// Earliest start time (seconds from now) at which new_vram GB
/// becomes free on an agent with the given total VRAM and active slots
/// [(finish_offset_seconds, vram_gb), ...]. Python `_earliest_start`.
fn earliest_start(slots: &[(f64, i64)], new_vram: i64, total_vram: i64) -> f64 {
    let used_now: i64 = slots.iter().map(|(_, v)| v).sum();
    let available_now = total_vram - used_now;
    if available_now >= new_vram {
        return 0.0;
    }
    let mut by_end: Vec<(f64, i64)> = slots.to_vec();
    by_end.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut freed = available_now;
    for (end_time, vram) in &by_end {
        freed += vram;
        if freed >= new_vram {
            return *end_time;
        }
    }
    by_end.last().map(|(end, _)| *end).unwrap_or(0.0)
}

/// Every GCP gpu_type whose required VRAM tier <= `local_vram_gb`.
/// Python `providers.local.helpers._compat_accel_types`, ported here so
/// the matcher can align optimizer-side assignment with agent-side
/// eligibility without pulling in the local-provider module.
fn compat_accel_types(local_vram_gb: i64) -> Vec<&'static str> {
    let mut accels: Vec<&'static str> = Vec::new();
    let Some(sizing) = GPU_SIZING.get(crate::capabilities::ProviderId::Gcp.as_str()) else {
        return accels;
    };
    for (tier, (_, accel)) in sizing {
        if local_vram_gb >= *tier && !accel.is_empty() && !accels.contains(accel) {
            accels.push(accel);
        }
    }
    accels
}

/// Pick the eligible worker that finishes this job earliest; update its active
/// jobs in place. Returns the chosen consumer_id, or `None` if no worker has
/// enough total VRAM.
/// Python `_assign_one`.
pub(super) fn assign_one(
    job: &Job,
    agents: &mut BTreeMap<String, AgentInfo>,
    runtime: f64,
    vram: i64,
) -> Option<String> {
    if job.exclusive {
        return None;
    }
    let mut best_cid: Option<String> = None;
    let mut best_finish: Option<f64> = None;
    for (cid, info) in agents.iter() {
        // Keep optimizer-side assignment aligned with agent-side eligibility.
        // A provider-pinned job assigned to a different consumer kind becomes
        // unclaimable: the pinned agent refuses it, and the assigned agent
        // also refuses it. Confirmed live with a gcp-pinned smoke assigned to
        // local-ubuntu-server.
        if job.pin_to_provider && job.provider != info.kind {
            continue;
        }
        if info.total_vram_gb < vram {
            continue;
        }
        let accel = job.gpu_type.as_str();
        if !accel.is_empty()
            && !info.available_accelerators.contains_key(accel)
            && !compat_accel_types(info.total_vram_gb).contains(&accel)
        {
            continue;
        }
        let start = earliest_start(&info.active_jobs, vram, info.total_vram_gb);
        let finish = start + runtime;
        if best_finish.is_none_or(|best| finish < best) {
            best_finish = Some(finish);
            best_cid = Some(cid.clone());
        }
    }
    let best_cid = best_cid?;
    let best_finish = best_finish?;
    agents
        .get_mut(&best_cid)?
        .active_jobs
        .push((best_finish, vram));
    Some(best_cid)
}
