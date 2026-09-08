//! Accelerator pricing lookup shared by the local-pack scoring pass and by
//! agent-VM bucketing's cost-cap check.

/// Return $/hour for one accelerator of this type at given pricing model.
/// Python `_accel_hourly_rate`.
pub fn accel_hourly_rate(accel_type: &str, preemptible: bool) -> f64 {
    let base = crate::catalog::GPU_HOURLY_RATE_USD
        .get(accel_type)
        .copied()
        .unwrap_or(0.0);
    if !preemptible {
        return base;
    }
    base * crate::catalog::SPOT_DISCOUNT
        .get(accel_type)
        .copied()
        .unwrap_or(0.5)
}
