//! The per-job cost row both collectors emit.

/// One finished job with wall-time + cost attribution. Python `rows` dict.
#[derive(Debug, Clone)]
pub struct CostRow {
    pub job_id: String,
    pub state: String,
    pub gpu_type: String,
    pub preemptible: bool,
    pub wall_s: f64,
    pub rate_usd_hr: f64,
    pub cost_usd: f64,
    pub target_kind: String,
    pub model: String,
}
