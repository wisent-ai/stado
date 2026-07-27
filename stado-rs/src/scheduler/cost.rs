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
/// instance_ref shape (`name@zone-or-location`) doesn't disambiguate cloud
/// providers, so we trust job.provider when set and fall back to the
/// "anything-but-local@ -> gcp" heuristic for older records that predate
/// multi-provider support.
///
/// Python `_target_kind`.
pub fn target_kind(job: &Job) -> String {
    let reference = job.instance_ref.as_deref().unwrap_or("");
    if reference.starts_with("local@") {
        return "local".into();
    }
    let provider = job.provider.trim();
    if matches!(provider, "azure" | "aws" | "gcp") {
        return provider.into();
    }
    if !reference.is_empty() {
        return "gcp".into();
    }
    "unknown".into()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn finished_job(
        job_id: &str,
        state_ended: (&str, &str),
        gpu_type: &str,
        machine_type: &str,
        preemptible: bool,
        instance_ref: &str,
        command: &str,
    ) -> Job {
        let mut job = Job::new(job_id, command);
        job.started_at = Some("2026-05-19T00:00:00+00:00".into());
        job.gpu_type = gpu_type.into();
        job.machine_type = machine_type.into();
        job.preemptible = preemptible;
        if !instance_ref.is_empty() {
            job.instance_ref = Some(instance_ref.into());
        }
        match state_ended {
            ("completed", ended) => {
                job.state = "completed".into();
                job.completed_at = Some(ended.into());
            }
            ("failed", ended) => {
                job.state = "failed".into();
                job.failed_at = Some(ended.into());
            }
            _ => unreachable!(),
        }
        job
    }

    #[test]
    fn hourly_rate_gcp_on_demand_spot_and_bundle() {
        // L4 on-demand: GPU 0.71 + g2-standard-4 bundle 0.30 = 1.01.
        assert_eq!(hourly_rate_usd("nvidia-l4", false, ""), 1.01);
        // L4 spot: 0.71*0.40 + 0.12 = 0.284 + 0.12 = 0.404.
        assert!((hourly_rate_usd("nvidia-l4", true, "") - 0.404).abs() < 1e-12);
        // Recorded machine_type wins over the catalog fallback.
        assert_eq!(
            hourly_rate_usd("nvidia-l4", false, "g2-standard-8"),
            0.71 + 0.60
        );
        // Unknown GPU, no machine type: zero.
        assert_eq!(hourly_rate_usd("mystery-gpu", false, ""), 0.0);
        // Unknown GPU default spot discount: 0.5x of 0.0 stays 0.0; check
        // the discount fallback on a known GPU without a SPOT_DISCOUNT
        // entry is not reachable from the catalog (all 20 entries have
        // both), so assert the A100 spot pair instead:
        // A100-80 spot: 3.67*0.54 + a2-ultragpu-1g 0.55 = 1.9818 + 0.55.
        assert!(
            (hourly_rate_usd("nvidia-a100-80gb", true, "") - (3.67 * 0.54 + 0.55)).abs() < 1e-9
        );
    }

    #[test]
    fn hourly_rate_azure_single_line_item() {
        // Azure NC* SKUs bundle the GPU: no GPU+bundle sum.
        assert_eq!(
            hourly_rate_usd("nvidia-a10", false, "Standard_NC8ads_A10_v4"),
            0.91
        );
        assert_eq!(
            hourly_rate_usd("nvidia-a10", true, "Standard_NC8ads_A10_v4"),
            0.18
        );
        // Unknown Azure size -> 0.
        assert_eq!(
            hourly_rate_usd("nvidia-a10", false, "Standard_UNKNOWN"),
            0.0
        );
    }

    #[test]
    fn target_kind_attribution() {
        let mut job = Job::new("j", "echo");
        job.instance_ref = Some("local@mac-mini".into());
        job.provider = "gcp".into(); // local@ wins over provider
        assert_eq!(target_kind(&job), "local");
        job.instance_ref = Some("wc-abc@us-central1-a".into());
        job.provider = "azure".into();
        assert_eq!(target_kind(&job), "azure");
        job.provider = "aws".into();
        assert_eq!(target_kind(&job), "aws");
        // Legacy record: unknown provider, non-local ref -> gcp heuristic.
        job.provider = String::new();
        assert_eq!(target_kind(&job), "gcp");
        job.instance_ref = None;
        assert_eq!(target_kind(&job), "unknown");
    }

    #[test]
    fn model_from_command_variants() {
        assert_eq!(model_from_command("x --model org/m --task t"), "org/m");
        assert_eq!(model_from_command("x --model 'org/q' --task t"), "org/q");
        assert_eq!(model_from_command("x --model \"org/d\""), "org/d");
        // Unterminated quote falls through to the bare-token path.
        assert_eq!(model_from_command("x --model 'org/u"), "org/u");
        assert_eq!(model_from_command("x --model"), "");
        assert_eq!(model_from_command("no flags"), "");
    }

    fn row(model: &str, gpu_type: &str, wall_s: f64) -> CostRow {
        CostRow {
            job_id: "j".into(),
            state: "completed".into(),
            gpu_type: gpu_type.into(),
            preemptible: false,
            wall_s,
            rate_usd_hr: 0.0,
            cost_usd: 0.0,
            target_kind: "local".into(),
            model: model.into(),
        }
    }

    #[test]
    fn wall_time_table_upper_median_and_estimation() {
        let rows = vec![
            row("m", "nvidia-l4", 100.0),
            row("m", "nvidia-l4", 300.0),
            row("m", "nvidia-l4", 200.0),
            row("m", "nvidia-l4", 400.0), // even count -> upper median 300
            row("", "nvidia-l4", 50.0),
        ];
        let table = wall_time_table(&rows);
        assert_eq!(table[&("m".to_string(), "nvidia-l4".to_string())], 300.0);
        assert_eq!(
            table[&("(unknown)".to_string(), "nvidia-l4".to_string())],
            50.0
        );

        // Observed median wins when present.
        assert_eq!(
            estimate_wall_time("x --model m", "nvidia-l4", 24, &table),
            300.0
        );
        // Missing (model, gpu_type) -> heuristic: 50 + 7*(80 + 24*5) = 1450.
        assert_eq!(
            estimate_wall_time("x --model other", "nvidia-l4", 24, &table),
            1450.0
        );
        assert_eq!(heuristic_wall_time_seconds(0), 50.0 + 7.0 * 80.0);
    }

    #[tokio::test]
    async fn report_aggregates_per_provider_with_spot_and_bundle() {
        let (_dir, store) = store();
        // 1h on local RTX PRO 6000 (owned hardware: 0.18 + no bundle).
        let local = finished_job(
            "j-local",
            ("completed", "2026-05-19T01:00:00+00:00"),
            "nvidia-rtx-pro-6000",
            "",
            false,
            "local@rtx-box",
            "x --model org/m1 --task t",
        );
        store.write_job("completed", &local).await.unwrap();
        // 30min on spot L4 on GCP: rate 0.404/hr -> 0.202.
        let spot = finished_job(
            "j-spot",
            ("completed", "2026-05-19T00:30:00+00:00"),
            "nvidia-l4",
            "",
            true,
            "wc-abc@us-central1-a",
            "x --model org/m2 --task t",
        );
        store.write_job("completed", &spot).await.unwrap();
        // 2h on Azure NC8ads_A10_v4 on-demand: 0.91/hr -> 1.82. Failed job
        // counts (wall from started -> failed_at).
        let azure = finished_job(
            "j-azure",
            ("failed", "2026-05-19T02:00:00+00:00"),
            "nvidia-a10",
            "Standard_NC8ads_A10_v4",
            false,
            "wc-def@eastus",
            "x --model org/m1 --task t",
        );
        let mut azure = azure;
        azure.provider = "azure".into(); // target_kind trusts job.provider when set
        store.write_job("failed", &azure).await.unwrap();
        // No wall-time (never started) -> excluded.
        let mut never = Job::new("j-never", "x --model org/m9 --task t");
        never.state = "completed".into();
        store.write_job("completed", &never).await.unwrap();

        let rep = report(&store).await.unwrap();
        assert_eq!(rep.total_jobs, 3);
        let expected = 0.18 + (0.404 / 2.0) + (0.91 * 2.0);
        assert!(
            (rep.total_cost_usd - expected).abs() < 1e-9,
            "{} vs {expected}",
            rep.total_cost_usd
        );
        assert!((rep.total_wall_s - (3600.0 + 1800.0 + 7200.0)).abs() < 1e-9);

        // by_target: local / gcp (heuristic for legacy-style ref) / azure.
        assert_eq!(rep.by_target["local"].jobs, 1);
        assert!((rep.by_target["local"].cost_usd - 0.18).abs() < 1e-9);
        assert_eq!(rep.by_target["gcp"].jobs, 1);
        assert!((rep.by_target["gcp"].cost_usd - 0.202).abs() < 1e-9);
        assert_eq!(rep.by_target["azure"].jobs, 1);
        assert!((rep.by_target["azure"].cost_usd - 1.82).abs() < 1e-9);
        // by_model: m1 aggregates the local + azure jobs.
        assert_eq!(rep.by_model["org/m1"].jobs, 2);
        assert!((rep.by_model["org/m1"].cost_usd - (0.18 + 1.82)).abs() < 1e-9);
        assert_eq!(rep.by_model["org/m2"].jobs, 1);

        // Rows carry the attribution fields.
        let spot_row = rep.rows.iter().find(|r| r.job_id == "j-spot").unwrap();
        assert!(spot_row.preemptible);
        assert!((spot_row.rate_usd_hr - 0.404).abs() < 1e-9);
        assert_eq!(spot_row.target_kind, "gcp");

        // Exact CLI output shape.
        let lines = format_report(&rep);
        assert_eq!(lines[0], "jobs_with_walltime: 3");
        assert_eq!(lines[1], format!("total_cost_usd:     ${expected:.4}"));
        assert_eq!(
            lines[2],
            format!("total_wall_hours:   {:.2}", 12600.0 / 3600.0)
        );
        assert_eq!(lines[3], "");
        assert_eq!(lines[4], "by target_kind:");
        assert!(lines
            .iter()
            .any(|l| l.starts_with(&format!("  {:<10} jobs=1", "azure"))));
        assert!(lines.iter().any(|l| l.starts_with("  org/m1")));
    }

    #[tokio::test]
    async fn project_batch_from_observed_average() {
        let (_dir, store) = store();
        // Two 1h jobs at 1.01/hr (on-demand L4).
        for id in ["j1", "j2"] {
            let job = finished_job(
                id,
                ("completed", "2026-05-19T01:00:00+00:00"),
                "nvidia-l4",
                "",
                false,
                "wc-x@us-central1-a",
                "x --model org/m --task t",
            );
            store.write_job("completed", &job).await.unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let batch = dir.path().join("batch.txt");
        // 3 real commands + a comment + a blank line.
        std::fs::write(&batch, "cmd a\ncmd b\n# comment\n\ncmd c\n").unwrap();
        let proj = project_batch(&batch, &store).await.unwrap();
        assert_eq!(proj.jobs_in_batch, 3);
        assert_eq!(proj.samples, 2);
        assert!((proj.avg_cost_usd_per_job - 1.01).abs() < 1e-9);
        assert!((proj.projected_cost_usd.unwrap() - 3.03).abs() < 1e-9);
        assert_eq!(proj.by_model["org/m"].jobs, 2);
    }

    #[tokio::test]
    async fn project_batch_without_history_cannot_project() {
        let (_dir, store) = store();
        let dir = tempfile::tempdir().unwrap();
        let batch = dir.path().join("batch.txt");
        std::fs::write(&batch, "cmd a\n").unwrap();
        let proj = project_batch(&batch, &store).await.unwrap();
        assert_eq!(proj.projected_cost_usd, None);
        assert_eq!(proj.reason, "no completed jobs to base projection on");
        assert_eq!(proj.jobs_in_batch, 0);
        assert_eq!(proj.samples, 0);
    }
}
