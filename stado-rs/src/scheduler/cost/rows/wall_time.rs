//! Wall-time medians over collected rows. A (model, gpu_type) pair with no
//! history has no estimate: callers take the job's stated
//! `runtime_seconds_estimate` or treat the run time as unknown, the way the
//! makespan matcher already refuses to guess.

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

/// The run time a job states (`runtime_seconds_estimate`, when it is a
/// positive number), else the median observed for this (model, gpu_type),
/// else `None`. Nothing is invented for a pair without history.
pub fn estimate_wall_time(
    job_command: &str,
    gpu_type: &str,
    stated_seconds: f64,
    table: &BTreeMap<(String, String), f64>,
) -> Option<f64> {
    if stated_seconds.is_normal() && stated_seconds.is_sign_positive() {
        return Some(stated_seconds);
    }
    let model = {
        let m = model_from_command(job_command);
        if m.is_empty() {
            "(unknown)".to_string()
        } else {
            m
        }
    };
    table
        .get(&(model, gpu_type.to_string()))
        .copied()
        .filter(|median| median.is_normal() && median.is_sign_positive())
}
