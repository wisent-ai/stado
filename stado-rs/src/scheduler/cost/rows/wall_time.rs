//! Wall-time medians over collected rows, plus the heuristic used when a
//! (model, gpu_type) pair has no history yet. Exposed for the local-pack
//! knapsack the scheduler runs.

use std::collections::BTreeMap;

use crate::scheduler::cost::measure::attribution::model_from_command;

use super::row::CostRow;

/// Median observed wall_time_seconds keyed by (model, gpu_type).
/// Python `wall_time_table` (upper median for even sample counts).
pub fn wall_time_table(rows: &[CostRow]) -> BTreeMap<(String, String), f64> {
    let mut buckets: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for r in rows {
        let model = if r.model.is_empty() {
            "(unknown)".to_string()
        } else {
            r.model.clone()
        };
        buckets
            .entry((model, r.gpu_type.clone()))
            .or_default()
            .push(r.wall_s);
    }
    let mut out = BTreeMap::new();
    for (key, mut walls) in buckets {
        walls.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        out.insert(key, walls[walls.len() / 2]);
    }
    out
}

/// Used when no completed-job data exists for a (model, gpu_type) pair.
///
/// Derived from cdacc255 phase data: 50s startup + 7 strategies, each strategy
/// spending ~80s on layer upload plus extract time scaling with model size.
/// Python `heuristic_wall_time_seconds`.
pub fn heuristic_wall_time_seconds(gpu_mem_gb: i64) -> f64 {
    let base = 50.0;
    let per_strategy = 80.0 + (gpu_mem_gb as f64 * 5.0).max(0.0);
    base + 7.0 * per_strategy
}

/// Median observed wall-time for this (model, gpu_type) when available.
/// Python `estimate_wall_time`.
pub fn estimate_wall_time(
    job_command: &str,
    gpu_type: &str,
    gpu_mem_gb: i64,
    table: &BTreeMap<(String, String), f64>,
) -> f64 {
    let model = {
        let m = model_from_command(job_command);
        if m.is_empty() {
            "(unknown)".to_string()
        } else {
            m
        }
    };
    if let Some(val) = table.get(&(model, gpu_type.to_string())) {
        if *val > 0.0 {
            return *val;
        }
    }
    heuristic_wall_time_seconds(gpu_mem_gb)
}
