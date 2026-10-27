//! Wall-time medians over collected rows. A (model, gpu_type) pair with no
//! history has no estimate: callers take the job's stated
//! `runtime_seconds_estimate` or treat the run time as unknown, the way the
//! makespan matcher already refuses to guess.

use std::collections::BTreeMap;

use super::row::CostRow;

/// Median observed wall seconds keyed by (the model a command names, or
/// `None` when it names none; the GPU type).
pub type WallTimes = BTreeMap<(Option<String>, String), f64>;

/// The upper median of the observed wall seconds of every (model, gpu_type)
/// pair the rows hold ([`crate::primitives::samples::upper_median`]).
pub fn wall_time_table(rows: &[CostRow]) -> WallTimes {
    let mut buckets: BTreeMap<(Option<String>, String), Vec<f64>> = BTreeMap::new();
    for row in rows {
        buckets
            .entry((row.model.clone(), row.gpu_type.clone()))
            .or_default()
            .push(row.wall_s);
    }
    buckets
        .into_iter()
        .filter_map(|(key, walls)| {
            crate::primitives::samples::upper_median(walls).map(|median| (key, median))
        })
        .collect()
}

/// The run time a job states (`runtime_seconds_estimate`, when it is a
/// positive number), else the median observed for the model its command
/// names ([`crate::sizing::model_of`]) on this GPU type, else `None`.
/// Nothing is invented for a pair without history.
pub fn estimate_wall_time(
    job_command: &str,
    gpu_type: &str,
    stated_seconds: f64,
    table: &WallTimes,
) -> Option<f64> {
    if stated_seconds.is_normal() && stated_seconds.is_sign_positive() {
        return Some(stated_seconds);
    }
    table
        .get(&(crate::sizing::model_of(job_command), gpu_type.to_string()))
        .copied()
        .filter(|median| median.is_normal() && median.is_sign_positive())
}
