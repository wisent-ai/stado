//! Statistics over observed samples, read the one way every Stado estimate
//! reads them.

/// The upper median of `values`, `None` when there are none: walking the
/// sorted values from both ends, the first place the low side reaches the
/// high side, which for an even count is the upper of the two middle values.
/// Floats sort by their total order, so every sample has a place and no
/// comparison is left undecided.
pub(crate) fn upper_median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    values
        .iter()
        .zip(values.iter().rev())
        .find(|(low, high)| low >= high)
        .map(|(middle, _)| *middle)
}
