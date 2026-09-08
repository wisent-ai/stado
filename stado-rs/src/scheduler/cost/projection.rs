//! Batch cost projection: observed per-job cost times the job count in a
//! batch file.

use std::collections::BTreeMap;
use std::path::Path;

use crate::queue::{JobStorage, StorageError};

use super::summary::{report, BucketSummary};

/// Python `project_batch` result. `projected_cost_usd` is None when there
/// is no completed-job data to base the projection on.
#[derive(Debug, Default)]
pub struct Projection {
    pub jobs_in_batch: usize,
    pub samples: usize,
    pub avg_cost_usd_per_job: f64,
    pub projected_cost_usd: Option<f64>,
    pub by_model: BTreeMap<String, BucketSummary>,
    pub reason: String,
}

/// Project total cost for a batch file, using observed per-job cost.
/// Python `project_batch`.
pub async fn project_batch(
    batch_path: &Path,
    store: &JobStorage,
) -> Result<Projection, StorageError> {
    let rep = report(store).await?;
    let n_rows = rep.rows.len();
    if n_rows == 0 {
        return Ok(Projection {
            jobs_in_batch: 0,
            samples: 0,
            projected_cost_usd: None,
            reason: "no completed jobs to base projection on".into(),
            ..Default::default()
        });
    }
    let avg = rep.total_cost_usd / n_rows as f64;
    let text = std::fs::read_to_string(batch_path).map_err(|e| {
        StorageError::Other(format!(
            "cannot read batch file {}: {e}",
            batch_path.display()
        ))
    })?;
    // Python: `line.strip() and not line.startswith("#")` — the #-check is
    // on the RAW line, so an indented "# comment" line still counts.
    let n = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .count();
    Ok(Projection {
        jobs_in_batch: n,
        samples: n_rows,
        avg_cost_usd_per_job: avg,
        projected_cost_usd: Some(avg * n as f64),
        by_model: rep.by_model,
        ..Default::default()
    })
}
