//! The two write paths: OOM escalation up the real GPU ladder and the
//! coordinator-authoritative queue sizing pass.

use crate::models::{job_state, Job};
use crate::queue::{JobStorage, StorageError};

use super::{is_oom_error, model_of, oom_required_gb, Sizing};

impl Sizing {
    /// A job that OOMed while sized at some GPU — and has NO measured
    /// peak yet — is moved to the next-larger REAL GPU currently in the
    /// fleet and requeued (running -> queue) instead of failed. Returns
    /// true iff requeued. If no live GPU is larger, or the model already
    /// has a measured peak, this does nothing and the caller fails it.
    ///
    /// No hand-written tier ladder: the next size comes from the actual
    /// GPUs the fleet is broadcasting (next_live_vram). An unmeasured model
    /// starts on the smallest real fleet GPU and climbs the real observed
    /// GPUs one OOM at a time until it runs; that run's measured nvidia-smi
    /// peak then sizes every later job of the model.
    /// Python `escalate_on_oom`.
    pub async fn escalate_on_oom(
        &self,
        store: &JobStorage,
        job: &mut Job,
        error_text: &str,
    ) -> Result<bool, StorageError> {
        if !is_oom_error(error_text) {
            return Ok(false);
        }
        let model = model_of(&job.command);
        if model.is_empty() {
            return Ok(false);
        }
        let cur = job.gpu_mem_gb;
        let measured_floor = oom_required_gb(error_text);
        let nxt = if measured_floor > cur {
            let live_vrams = self.live_total_vrams(store).await?;
            if !live_vrams.is_empty() && measured_floor > *live_vrams.last().unwrap_or(&0) {
                return Ok(false);
            }
            Some(measured_floor)
        } else {
            if self.observed_vram_gb(store, &model).await?.is_some() {
                return Ok(false); // measured already; a real OOM is a real failure
            }
            self.next_live_vram(store, cur).await?
        };
        let Some(nxt) = nxt else {
            return Ok(false); // no live GPU bigger than current — genuine failure
        };
        job.gpu_mem_gb = nxt;
        job.state = job_state::QUEUED.into();
        job.failed_at = None;
        job.error = None;
        job.instance_ref = None;
        job.started_at = None;
        store.move_job(job, "running", "queue").await?;
        store.cleanup_status(&job.job_id).await?;
        Ok(true)
    }

    /// Coordinator-authoritative sizing pass, run once per tick BEFORE
    /// assignment. Python `normalize_queue_sizing`.
    ///
    /// A queued job's gpu_mem_gb is owned by the sizing path, not by the
    /// agent that last touched it. An agent still on pre-0.4.237
    /// wisent-compute (not yet drifted) requeues jobs writing the OLD
    /// hardcoded estimate_gpu_memory output (gpt-oss-20b -> 64/12/80); the
    /// 0.4.238 makespan apply-assignment then faithfully PRESERVES that
    /// stale value because it only rewrites assigned_to. So the queue keeps
    /// re-accumulating hardcoded sizes until every agent has drifted.
    ///
    /// This pass closes that gap structurally: for every queued job whose
    /// model has NO measured peak yet, force gpu_mem_gb back to 0 — the
    /// canonical "no stored size, sized live at claim time" state. For a
    /// model WITH a measured peak, stamp that measured peak (the ground
    /// truth). Either way the stored number is never a hardcoded guess.
    /// A lagging agent's stale write is corrected within one tick instead
    /// of persisting until fleet-wide drift completes.
    ///
    /// Fresh read-modify-write of ONLY gpu_mem_gb so a concurrent
    /// makespan assigned_to write on the same blob is not lost. Returns
    /// the number of queue blobs corrected this tick.
    pub async fn normalize_queue_sizing(
        &self,
        store: &JobStorage,
        log_fn: &dyn Fn(&str),
    ) -> Result<usize, StorageError> {
        let mut corrected = 0usize;
        for job in store.list_jobs("queue", 0).await? {
            let model = model_of(&job.command);
            if model.is_empty() {
                continue;
            }
            let peak = self.observed_vram_gb(store, &model).await?;
            let desired = peak.unwrap_or(0);
            if job.gpu_mem_gb == desired {
                continue;
            }
            if store
                .update_queued_gpu_mem(&job.job_id, desired)
                .await?
                .is_some()
            {
                corrected += 1;
            }
        }
        if corrected > 0 {
            log_fn(&format!(
                "sizing: normalized {corrected} queue jobs \
                 (unmeasured->0 / measured->peak); stale-agent clobber corrected"
            ));
        }
        Ok(corrected)
    }
}
