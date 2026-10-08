//! Aggregation of finished-job rows into per-bucket summaries, and the
//! exact `stado cost report` output lines rendered from them.

use std::collections::BTreeMap;

use crate::queue::{JobStorage, StorageError};

use super::rows::collect::collect_completed;
use super::rows::row::CostRow;

/// Aggregation bucket. Python `{"jobs","wall_s","cost_usd"}` dicts, plus the
/// jobs no live quote priced: their cost is not in `cost_usd`.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct BucketSummary {
    pub jobs: usize,
    pub wall_s: f64,
    pub cost_usd: f64,
    pub unpriced_jobs: usize,
}

/// Python `report` result.
#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub rows: Vec<CostRow>,
    pub by_target: BTreeMap<String, BucketSummary>,
    pub by_model: BTreeMap<String, BucketSummary>,
    pub total_jobs: usize,
    pub total_cost_usd: f64,
    pub total_unpriced_jobs: usize,
    pub total_wall_s: f64,
}

/// Aggregate finished-job rows into per-bucket summaries. Python `report`.
pub async fn report(store: &JobStorage) -> Result<Report, StorageError> {
    let rows = collect_completed(store).await?;
    let mut rep = Report {
        total_jobs: rows.len(),
        ..Default::default()
    };
    for r in &rows {
        for (table, key) in [
            (&mut rep.by_target, r.target_kind.clone()),
            (
                &mut rep.by_model,
                if r.model.is_empty() {
                    "(unknown)".to_string()
                } else {
                    r.model.clone()
                },
            ),
        ] {
            let bucket = table.entry(key).or_default();
            bucket.jobs += 1;
            bucket.wall_s += r.wall_s;
            if let Some(cost) = r.cost_usd {
                bucket.cost_usd += cost;
            }
            bucket.unpriced_jobs += usize::from(r.cost_usd.is_none());
        }
        if let Some(cost) = r.cost_usd {
            rep.total_cost_usd += cost;
        }
        rep.total_unpriced_jobs += usize::from(r.cost_usd.is_none());
        rep.total_wall_s += r.wall_s;
    }
    rep.rows = rows;
    Ok(rep)
}

/// Python `format_report` — the exact `stado cost report` output lines.
pub fn format_report(rep: &Report) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("jobs_with_walltime: {}", rep.total_jobs));
    lines.push(format!("total_cost_usd:     ${:.4}", rep.total_cost_usd));
    lines.push(format!("unpriced_jobs:      {}", rep.total_unpriced_jobs));
    lines.push(format!(
        "total_wall_hours:   {:.2}",
        rep.total_wall_s / 3600.0
    ));
    lines.push(String::new());
    lines.push("by target_kind:".to_string());
    for (k, v) in &rep.by_target {
        lines.push(format!(
            "  {k:<10} jobs={:<5} wall_h={:>7.2} cost=${:.4} unpriced={}",
            v.jobs,
            v.wall_s / 3600.0,
            v.cost_usd,
            v.unpriced_jobs
        ));
    }
    lines.push(String::new());
    lines.push("by model:".to_string());
    for (k, v) in &rep.by_model {
        lines.push(format!(
            "  {k:<48} jobs={:<5} cost=${:.4} unpriced={}",
            v.jobs, v.cost_usd, v.unpriced_jobs
        ));
    }
    lines
}
