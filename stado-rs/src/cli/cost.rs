//! `stado cost report` / `stado cost estimate BATCH_FILE` — port of the
//! `cost` group in `stado/cli.py`, backed by
//! [`crate::scheduler::cost`].

use std::path::Path;

use crate::queue::submit::default_store;
use crate::queue::{JobStorage, StorageError};
use crate::scheduler::cost;

use super::{CmdError, CostCommands};

impl From<crate::scheduler::cost::Projection> for Vec<String> {
    /// Python cli.py `cost_estimate` echo lines.
    fn from(proj: crate::scheduler::cost::Projection) -> Self {
        let Some(projected) = proj.projected_cost_usd else {
            return vec![format!("cannot project: {}", proj.reason)];
        };
        vec![
            format!("jobs_in_batch:        {}", proj.jobs_in_batch),
            format!("samples:              {} completed jobs in queue history", proj.samples),
            format!("avg_cost_usd_per_job: ${:.4}", proj.avg_cost_usd_per_job),
            format!("projected_cost_usd:   ${projected:.2}"),
        ]
    }
}

/// Python `cost_report`: the formatted report lines.
pub(crate) async fn report_lines(store: &JobStorage) -> Result<Vec<String>, StorageError> {
    Ok(cost::format_report(&cost::report(store).await?))
}

/// Python `cost_estimate`: the projection lines ("cannot project: ..."
/// when there is no history to base the projection on).
pub(crate) async fn estimate_lines(
    store: &JobStorage,
    batch_file: &Path,
) -> Result<Vec<String>, StorageError> {
    Ok(cost::project_batch(batch_file, store).await?.into())
}

pub(crate) async fn dispatch(sub: &CostCommands) -> Result<(), CmdError> {
    let store = default_store(crate::config::bucket()).await?;
    match sub {
        CostCommands::Report => {
            for line in report_lines(&store).await? {
                println!("{line}");
            }
        }
        CostCommands::Estimate { batch_file } => {
            for line in estimate_lines(&store, Path::new(batch_file)).await? {
                println!("{line}");
            }
        }
    }
    Ok(())
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

    fn completed(job_id: &str, gpu_type: &str, preemptible: bool, command: &str) -> crate::models::Job {
        let mut job = crate::models::Job::new(job_id, command);
        job.state = "completed".into();
        job.started_at = Some("2026-05-19T00:00:00+00:00".into());
        job.completed_at = Some("2026-05-19T01:00:00+00:00".into());
        job.gpu_type = gpu_type.into();
        job.preemptible = preemptible;
        job.instance_ref = Some("wc-x@us-central1-a".into());
        job
    }

    #[tokio::test]
    async fn cost_report_lines_against_fabricated_blobs() {
        let (_dir, store) = store();
        store
            .write_job("completed", &completed("j1", "nvidia-l4", false, "x --model org/m --task t"))
            .await
            .unwrap();
        store
            .write_job("completed", &completed("j2", "nvidia-l4", true, "x --model org/m --task t"))
            .await
            .unwrap();

        let lines = report_lines(&store).await.unwrap();
        // j1 on-demand 1.01 + j2 spot 0.404 = 1.414 total, 2 wall-hours.
        assert_eq!(lines[0], "jobs_with_walltime: 2");
        assert_eq!(lines[1], "total_cost_usd:     $1.4140");
        assert_eq!(lines[2], "total_wall_hours:   2.00");
        assert_eq!(lines[3], "");
        assert_eq!(lines[4], "by target_kind:");
        assert_eq!(lines[5], "  gcp        jobs=2     wall_h=   2.00 cost=$1.4140");
        assert_eq!(lines[6], "");
        assert_eq!(lines[7], "by model:");
        assert_eq!(
            lines[8],
            format!("  {:<48} jobs=2     cost=$1.4140", "org/m")
        );
        assert_eq!(lines.len(), 9);
    }

    #[tokio::test]
    async fn cost_estimate_lines_against_fabricated_blobs() {
        let (_dir, store) = store();
        store
            .write_job("completed", &completed("j1", "nvidia-l4", false, "x --model org/m --task t"))
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let batch = dir.path().join("batch.txt");
        std::fs::write(&batch, "cmd a\ncmd b\n").unwrap();

        let lines = estimate_lines(&store, &batch).await.unwrap();
        assert_eq!(
            lines,
            vec![
                "jobs_in_batch:        2".to_string(),
                "samples:              1 completed jobs in queue history".to_string(),
                "avg_cost_usd_per_job: $1.0100".to_string(),
                "projected_cost_usd:   $2.02".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn cost_estimate_without_history_says_cannot_project() {
        let (_dir, store) = store();
        let dir = tempfile::tempdir().unwrap();
        let batch = dir.path().join("batch.txt");
        std::fs::write(&batch, "cmd a\n").unwrap();
        let lines = estimate_lines(&store, &batch).await.unwrap();
        assert_eq!(
            lines,
            vec!["cannot project: no completed jobs to base projection on".to_string()]
        );
    }
}
