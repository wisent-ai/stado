//! Whether the queue currently holds anything this host could take.
//!
//! The agent asks before it idles or shuts itself down, so the answer has to
//! apply the same fit and eligibility rules a real claim would.

use crate::config::estimate_gpu_memory;
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

use super::eligibility::job_eligible;

/// True when no queued job fits + is eligible for this consumer.
/// Python `_no_eligible_in_queue`.
#[allow(clippy::too_many_arguments)]
pub async fn no_eligible_in_queue(
    store: &JobStorage,
    sizing: &Sizing,
    gpu_type: &str,
    total_vram_gb: i64,
    free_vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    active_job_count: usize,
) -> Result<bool, StorageError> {
    let queued = store.list_jobs("queue", 0).await?;
    for job in queued {
        let need = job
            .gpu_mem_gb
            .max(estimate_gpu_memory(&job.command, sizing, store).await?);
        if need > free_vram_gb {
            continue;
        }
        if !job_eligible(
            &job,
            gpu_type,
            total_vram_gb,
            kind,
            consumer_id,
            active_job_count,
            false,
        ) {
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}
