//! Pass two: the cost-optimal local pack.

use std::collections::{BTreeMap, HashMap};

use crate::models::Job;
use crate::scheduler::cost;
use crate::scheduler::scheduler::support::rates::accel_hourly_rate;
use crate::scheduler::scheduler::support::reporting::{log, py_pairs_i64};

/// The cost-optimal local-pack knapsack half of Python
/// `schedule_queued_jobs`, split out for tests. Returns job_id ->
/// consumer_id yields.
///
/// `local_vram_pool` is each agent's claimable VRAM — its free VRAM less the
/// safety buffer it publishes — so a job is never yielded to an agent its own
/// admission rule would refuse.
///
/// COST-OPTIMAL LOCAL PACK: knapsack over queued jobs by
/// $-saved-per-GB-of-local-VRAM, weighted by per-job wall-time so the
/// score reflects total dollars-saved-per-GB on this specific job (not
/// per-hour-of-running). Wall-time is the job's stated
/// `runtime_seconds_estimate` or the median of past completed jobs of the
/// same (model, gpu_type). A job with neither has no score: it is packed
/// after every scored job, in queue order, rather than ranked by a guessed
/// run time. Best-fit-decreasing packing.
pub(crate) fn local_pack(
    queued: &[Job],
    local_vram_pool: &[(String, i64)],
    wt_table: &BTreeMap<(String, String), f64>,
) -> HashMap<String, String> {
    let mut yield_targets: HashMap<String, String> = HashMap::new();
    if local_vram_pool.is_empty() {
        return yield_targets;
    }
    // `None` scores (no stated or measured run time) sort after every
    // measured one; among themselves they keep queue order.
    let mut scored: Vec<(Option<f64>, i64, &Job)> = Vec::new();
    for j in queued {
        let need = j.gpu_mem_gb;
        if need <= 0 || j.pin_to_provider {
            continue;
        }
        let rate = accel_hourly_rate(&j.gpu_type, j.preemptible);
        if rate <= 0.0 {
            continue;
        }
        // $-saved per GB on this job.
        let score = cost::estimate_wall_time(
            &j.command,
            &j.gpu_type,
            j.runtime_seconds_estimate,
            wt_table,
        )
        .map(|wall_s| {
            (wall_s / crate::monitor::billing::SECONDS_PER_HOUR as f64) * rate / need as f64
        });
        scored.push((score, need, j));
    }
    scored.sort_by(|a, b| match (a.0, b.0) {
        (Some(left), Some(right)) => right.total_cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    // (consumer_id, claimable VRAM) in the pool's original order
    // (consumers_by_claimable_vram sorts desc); best-fit picks the
    // strictly-largest free entry so iteration order breaks ties exactly
    // like the Python dict scan.
    let mut local_remaining: Vec<(String, i64)> = local_vram_pool.to_vec();
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
