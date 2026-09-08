//! The inference reservation a host may be holding its GPU for, and the
//! queued GPU work that decides whether the reservation keeps it.

use std::time::Duration;

use crate::config::estimate_gpu_memory;
use crate::providers::local::helpers;
use crate::providers::local::slots::job_system_packages_eligible;
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

use super::super::{CLAIM_CANDIDATE_WINDOW, QUEUE_SCAN_BUDGET};

// An admission decision needs the whole picture at once: store, sizing, device,
// capacity and kind are each read on a different branch below.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn queued_gpu_job_for_inference(
    store: &JobStorage,
    sizing: &Sizing,
    gpu_type: &str,
    total_vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    active_job_count: usize,
    pinned_only: bool,
) -> Result<Option<(String, i64)>, StorageError> {
    let listed = store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: CLAIM_CANDIDATE_WINDOW,
                scan_budget: QUEUE_SCAN_BUDGET,
                max_gpu_mem_gb: total_vram_gb,
                eligible: &|job| {
                    helpers::job_eligible(
                        job,
                        gpu_type,
                        total_vram_gb,
                        kind,
                        consumer_id,
                        active_job_count,
                        pinned_only,
                    ) && job_system_packages_eligible(job, kind)
                },
                // A claim loop wants reachability and so takes the shared
                // rotation: a job past this poll's window is reached by a
                // later poll rather than never.
                from_head: false,
            },
        )
        .await?;
    let mut queued = Vec::with_capacity(listed.len());
    for candidate in listed {
        if let Some(job) = store.read_job("queue", &candidate.job_id).await? {
            queued.push(job);
        }
    }
    queued.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.created_at.cmp(&right.created_at))
    });
    for job in queued {
        let need = job
            .gpu_mem_gb
            .max(estimate_gpu_memory(&job.command, sizing, store).await?);
        if need <= 0
            || !helpers::job_eligible(
                &job,
                gpu_type,
                total_vram_gb,
                kind,
                consumer_id,
                active_job_count,
                pinned_only,
            )
            || !job_system_packages_eligible(&job, kind)
        {
            continue;
        }
        return Ok(Some((job.job_id, need)));
    }
    Ok(None)
}

fn inference_container_name(deployment: &str) -> Result<String, String> {
    let valid = !deployment.is_empty()
        && deployment.len() <= 128
        && deployment.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_".contains(&byte)
        });
    valid
        .then(|| format!("stado-inference-{deployment}"))
        .ok_or_else(|| "inference reservation contains an invalid deployment name".to_string())
}

pub(crate) async fn inference_container_running(deployment: &str) -> Result<bool, String> {
    let container = inference_container_name(deployment)?;
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new("docker")
            .args(["inspect", "--format={{.State.Running}}", &container])
            .output(),
    )
    .await
    .map_err(|_| "docker inspect timed out".to_string())?
    .map_err(|error| format!("docker inspect failed: {error}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

pub(crate) async fn set_inference_container_running(
    deployment: &str,
    running: bool,
) -> Result<(), String> {
    let container = inference_container_name(deployment)?;
    let mut command = tokio::process::Command::new("docker");
    if running {
        command.args(["start", &container]);
    } else {
        command.args(["stop", "--time", "30", &container]);
    }
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .map_err(|_| "docker inference transition timed out".to_string())?
        .map_err(|error| format!("docker inference transition failed: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "docker inference transition exited {}: {}",
        output.status,
        detail.trim().chars().take(400).collect::<String>()
    ))
}
