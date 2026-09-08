//! Pass two: the cost-optimal local pack.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use crate::models::Job;
use crate::scheduler::cost;
use crate::scheduler::scheduler::support::pacing::backoff_due;
use crate::scheduler::scheduler::support::rates::accel_hourly_rate;
use crate::scheduler::scheduler::support::reporting::{log, py_pairs_i64};

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
