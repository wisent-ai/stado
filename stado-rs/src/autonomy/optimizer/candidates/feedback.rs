//! What the recorded placements say about a target.
//!
//! The startup time and the failure ratio a target actually showed, from the
//! feedback records the pass read. Both are statistics over the samples for
//! one target, so both answer `None` while there is nothing to measure.

pub(super) fn observed_startup_seconds(
    feedback: &[super::storage::PlacementFeedback],
    target: &str,
) -> Option<f64> {
    crate::primitives::samples::upper_median(
        feedback
            .iter()
            .filter(|entry| entry.target_id == target)
            .filter_map(|entry| entry.startup_seconds)
            .collect(),
    )
}

pub(super) fn observed_failure_probability(
    feedback: &[super::storage::PlacementFeedback],
    target: &str,
) -> Option<f64> {
    let samples: Vec<_> = feedback
        .iter()
        .filter(|entry| entry.target_id == target)
        .collect();
    if samples.is_empty() {
        return None;
    }
    let failed = samples.iter().filter(|entry| !entry.succeeded).count();
    Some(failed as f64 / samples.len() as f64)
}
