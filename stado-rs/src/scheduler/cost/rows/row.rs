//! The per-job cost row the collector emits.

/// One finished job with wall-time + cost attribution. Python `rows` dict.
/// `rate_usd_hr` and `cost_usd` are `None` when no live quote (or, for owned
/// hardware, no declared local rate) prices the job.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CostRow {
    pub job_id: String,
    pub state: String,
    pub gpu_type: String,
    pub preemptible: bool,
    pub wall_s: f64,
    pub rate_usd_hr: Option<f64>,
    pub cost_usd: Option<f64>,
    /// The provider the job ran on; `None` when the job records none.
    pub target_kind: Option<String>,
    /// The model the job's command names (`--model`, read by
    /// [`crate::sizing::model_of`]); `None` when it names none.
    pub model: Option<String>,
}
