//! Cost reporting + projection. Backs `stado cost report` and
//! `stado cost estimate`.
//!
//! Reads every job from JobStorage (completed/failed), computes wall-time
//! from started_at -> (completed_at or failed_at), looks up the matching
//! $/hour from catalog GPU_HOURLY_RATE_USD with SPOT_DISCOUNT applied when
//! preemptible=True, attributes each job by instance_ref (local@host vs
//! gcp:zone:instance), and aggregates per (gpu_type, target_kind, model_id).
//!
//! Replaces hand-waved cost ceilings with measured per-job distributions as
//! soon as any jobs have actually run.
//!
//! Port of `stado/scheduler/cost.py`. The wall-time medians
//! ([`wall_time_table`], [`estimate_wall_time`], [`heuristic_wall_time_seconds`])
//! are exposed for the local-pack knapsack used by the scheduler.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::catalog::{
    AZURE_VM_HOURLY_RATE_USD, GPU_HOURLY_RATE_USD, GPU_TYPE_TO_MACHINE_TYPE, SPOT_DISCOUNT,
    VM_BUNDLE_HOURLY_RATE_USD,
};
use crate::models::Job;
use crate::queue::{JobStorage, StorageError};

/// Parse an ISO-8601 timestamp with a trailing "Z" or offset. Python
/// `_parse_iso` (`fromisoformat(ts.replace("Z", "+00:00"))`).
fn parse_iso(ts: Option<&str>) -> Option<DateTime<Utc>> {
    let ts = ts?;
    if ts.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// started -> (completed or failed). None if either side missing.
/// Python `_wall_seconds`.
fn wall_seconds(job: &Job) -> Option<f64> {
    let start = parse_iso(job.started_at.as_deref())?;
    let end =
        parse_iso(job.completed_at.as_deref()).or_else(|| parse_iso(job.failed_at.as_deref()))?;
    Some(((end - start).num_milliseconds() as f64 / 1000.0).max(0.0))
}

/// Total $/hr the project actually pays for one VM.
///
/// Azure: NC* SKUs bundle the GPU into a single line item, so we read
/// AZURE_VM_HOURLY_RATE_USD directly and skip the GPU+bundle sum.
///
/// GCP: bills the GPU SKU and the A2/N1/G2 Core+Ram SKUs separately, so
/// summing both yields the line-item total a user sees in Cloud Billing.
/// Falls back to GPU_TYPE_TO_MACHINE_TYPE to look up the bundle when
/// machine_type wasn't recorded on the Job.
///
/// Python `_hourly_rate_usd`.
pub fn hourly_rate_usd(gpu_type: &str, preemptible: bool, machine_type: &str) -> f64 {
    if machine_type.starts_with("Standard_") {
        let (on_demand, spot) = AZURE_VM_HOURLY_RATE_USD
            .get(machine_type)
            .copied()
            .unwrap_or((0.0, 0.0));
        return if preemptible { spot } else { on_demand };
    }
    let mut gpu = GPU_HOURLY_RATE_USD.get(gpu_type).copied().unwrap_or(0.0);
    if preemptible {
        gpu *= SPOT_DISCOUNT.get(gpu_type).copied().unwrap_or(0.5);
    }
    let mt = if machine_type.is_empty() {
        GPU_TYPE_TO_MACHINE_TYPE
            .get(gpu_type)
            .copied()
            .unwrap_or("")
    } else {
        machine_type
    };
    let bundle_pair = VM_BUNDLE_HOURLY_RATE_USD
        .get(mt)
        .copied()
        .unwrap_or((0.0, 0.0));
    let bundle = if preemptible {
        bundle_pair.1
    } else {
        bundle_pair.0
    };
    gpu + bundle
}

/// local | gcp | azure | aws | unknown.
///
/// Explicit provider metadata wins. Legacy GCE records are recognized only
/// when their location has the unambiguous `region-zone` suffix; arbitrary
/// non-local references are never silently relabeled as GCP.
///
/// Python `_target_kind`.
pub fn target_kind(job: &Job) -> String {
    let reference = job.instance_ref.as_deref().unwrap_or("");
    if crate::capabilities::ProviderId::infer_from_instance_reference(reference)
        == Some(crate::capabilities::ProviderId::Local)
    {
        return crate::capabilities::ProviderId::Local.as_str().to_string();
    }
    let configured = crate::capabilities::variant(
        crate::capabilities::RuntimeFacet::Compute,
        job.provider.trim(),
    )
    .filter(|variant| {
        matches!(
            variant.adapter,
            crate::capabilities::RuntimeAdapter::Compute(adapter)
                if adapter.tracks_cloud_cost()
        )
    });
    if let Some(variant) = configured {
        return variant.id.to_string();
    }
    crate::capabilities::ProviderId::infer_from_instance_reference(reference)
        .map(|provider| provider.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Best-effort: extract --model 'X' out of the command line.
/// Python `_model_from_command`.
pub fn model_from_command(cmd: &str) -> String {
    let Some((_, rest)) = cmd.split_once("--model") else {
        return String::new();
    };
    let parts = rest.trim_start();
    let Some(first) = parts.chars().next() else {
        return String::new();
    };
    if first == '\'' || first == '"' {
        let after = &parts[first.len_utf8()..];
        if after.contains(first) {
            return after.split(first).next().unwrap_or("").to_string();
        }
        // Unterminated quote: Python falls through to the bare-token path.
        return after.split_whitespace().next().unwrap_or("").to_string();
    }
    parts.split_whitespace().next().unwrap_or("").to_string()
}

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

/// One entry per finished job with wall-time + cost attribution.
/// Python `collect_completed`.
pub async fn collect_completed(store: &JobStorage) -> Result<Vec<CostRow>, StorageError> {
    let mut rows = Vec::new();
    for state in ["completed", "failed"] {
        for job in store.list_jobs(state, 0).await? {
            let Some(wall) = wall_seconds(&job) else {
                continue;
            };
            let rate = hourly_rate_usd(&job.gpu_type, job.preemptible, &job.machine_type);
            let cost = (wall / 3600.0) * rate;
            rows.push(CostRow {
                job_id: job.job_id.clone(),
                state: state.into(),
                gpu_type: if job.gpu_type.is_empty() {
                    "cpu".into()
                } else {
                    job.gpu_type.clone()
                },
                preemptible: job.preemptible,
                wall_s: wall,
                rate_usd_hr: rate,
                cost_usd: cost,
                target_kind: target_kind(&job),
                model: model_from_command(&job.command),
            });
        }
    }
    Ok(rows)
}

/// Dynamic-price attribution for the autonomous control plane. Unlike the
/// legacy parity report, this never substitutes catalog constants for a live
/// cloud quote: an unpriced cloud row remains unmeasured.
pub async fn collect_completed_dynamic(store: &JobStorage) -> Result<Vec<CostRow>, StorageError> {
    let Some(prices) = crate::autonomy::storage::read_json::<crate::autonomy::cost::PriceBook>(
        store,
        "autonomy/cost/prices.json",
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for state in ["completed", "failed"] {
        for job in store.list_jobs(state, usize::default()).await? {
            let Some(wall) = wall_seconds(&job) else {
                continue;
            };
            let target = target_kind(&job);
            let provider = match target.as_str() {
                "gcp" => crate::capabilities::ProviderId::Gcp,
                "aws" => crate::capabilities::ProviderId::Aws,
                "azure" => crate::capabilities::ProviderId::Azure,
                "local" => crate::capabilities::ProviderId::Local,
                "box" => crate::capabilities::ProviderId::Box,
                "vast" => crate::capabilities::ProviderId::Vast,
                _ => continue,
            };
            let rate = if provider == crate::capabilities::ProviderId::Local {
                f64::default()
            } else {
                let Some(quote) = prices.find_hourly(
                    provider,
                    Some(job.region.as_str()).filter(|region| !region.is_empty()),
                    &job.machine_type,
                    &job.gpu_type,
                    job.preemptible,
                ) else {
                    continue;
                };
                quote.hourly_usd
            };
            let cost = wall / crate::monitor::billing::SECONDS_PER_HOUR as f64 * rate;
            rows.push(CostRow {
                job_id: job.job_id.clone(),
                state: state.into(),
                gpu_type: if job.gpu_type.is_empty() {
                    "cpu".into()
                } else {
                    job.gpu_type.clone()
                },
                preemptible: job.preemptible,
                wall_s: wall,
                rate_usd_hr: rate,
                cost_usd: cost,
                target_kind: target,
                model: model_from_command(&job.command),
            });
        }
    }
    Ok(rows)
}

/// Aggregation bucket. Python `{"jobs","wall_s","cost_usd"}` dicts.
#[derive(Debug, Default, Clone)]
pub struct BucketSummary {
    pub jobs: usize,
    pub wall_s: f64,
    pub cost_usd: f64,
}

/// Python `report` result.
#[derive(Debug, Default)]
pub struct Report {
    pub rows: Vec<CostRow>,
    pub by_target: BTreeMap<String, BucketSummary>,
    pub by_model: BTreeMap<String, BucketSummary>,
    pub total_jobs: usize,
    pub total_cost_usd: f64,
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
            bucket.cost_usd += r.cost_usd;
        }
        rep.total_cost_usd += r.cost_usd;
        rep.total_wall_s += r.wall_s;
    }
    rep.rows = rows;
    Ok(rep)
}

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

/// Python `format_report` — the exact `stado cost report` output lines.
pub fn format_report(rep: &Report) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("jobs_with_walltime: {}", rep.total_jobs));
    lines.push(format!("total_cost_usd:     ${:.4}", rep.total_cost_usd));
    lines.push(format!(
        "total_wall_hours:   {:.2}",
        rep.total_wall_s / 3600.0
    ));
    lines.push(String::new());
    lines.push("by target_kind:".to_string());
    for (k, v) in &rep.by_target {
        lines.push(format!(
            "  {k:<10} jobs={:<5} wall_h={:>7.2} cost=${:.4}",
            v.jobs,
            v.wall_s / 3600.0,
            v.cost_usd
        ));
    }
    lines.push(String::new());
    lines.push("by model:".to_string());
    for (k, v) in &rep.by_model {
        lines.push(format!(
            "  {k:<48} jobs={:<5} cost=${:.4}",
            v.jobs, v.cost_usd
        ));
    }
    lines
}
