//! The two write paths: OOM escalation up the real GPU ladder and the
//! coordinator-authoritative queue sizing pass.

use crate::models::{job_state, Job};
use crate::queue::{JobStorage, StorageError};

use super::{download_many, model_of, oom_required_gb, ran_out_of_memory, Sizing};

/// The queue directory the sizing pass reads.
const QUEUE_PREFIX: &str = "queue/";

impl Sizing {
    /// A job that OOMed while sized at some GPU — and has NO measured
    /// peak yet — is moved to the next-larger REAL GPU currently in the
    /// fleet and requeued (running -> queue) instead of failed. Returns
    /// true iff requeued. If no live GPU is larger, or the model already
    /// has a measured peak, this does nothing and the caller fails it.
    ///
    /// Whether the failure was for want of memory is read from PyTorch's
    /// own figures or, for any other error, by a model
    /// ([`ran_out_of_memory`]). An error nobody could read is not
    /// escalated: the job fails with its error, and the cause is said.
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
        let text = error_text.to_string();
        let judged = tokio::task::spawn_blocking(move || ran_out_of_memory(&text)).await;
        match judged {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return Ok(false),
            Ok(Err(cause)) => {
                eprintln!(
                    "sizing: job {} was not escalated: its error could not be read as an out-of-memory failure or not: {cause}",
                    job.job_id
                );
                return Ok(false);
            }
            Err(error) => {
                eprintln!(
                    "sizing: job {} was not escalated: reading its error stopped: {error}",
                    job.job_id
                );
                return Ok(false);
            }
        }
        let model = model_of(&job.command);
        if model.is_empty() {
            return Ok(false);
        }
        let cur = job.gpu_mem_gb;
        let nxt = match oom_required_gb(error_text).filter(|floor| *floor > cur) {
            Some(measured_floor) => {
                let live_vrams = self.live_total_vrams(store).await?;
                if live_vrams
                    .last()
                    .is_some_and(|largest| measured_floor > *largest)
                {
                    return Ok(false);
                }
                Some(measured_floor)
            }
            None => {
                if self.observed_vram_gb(store, &model).await?.is_some() {
                    return Ok(false); // measured already; a real OOM is a real failure
                }
                self.next_live_vram(store, cur).await?
            }
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
    /// hardcoded estimate_gpu_memory output; the makespan apply-assignment
    /// then faithfully PRESERVES that stale value because it only rewrites
    /// assigned_to. So the queue keeps re-accumulating hardcoded sizes
    /// until every agent has drifted.
    ///
    /// This pass closes that gap structurally: for every queued job whose
    /// model has NO measured peak yet, force gpu_mem_gb back to the job
    /// record's own unsized value — the canonical "no stored size, sized
    /// live at claim time" state. For a model WITH a measured peak, stamp
    /// that measured peak (the ground truth). Either way the stored number
    /// is never a hardcoded guess.
    ///
    /// Fresh read-modify-write of ONLY gpu_mem_gb so a concurrent
    /// makespan assigned_to write on the same blob is not lost. Returns
    /// the number of queue blobs corrected this tick.
    pub async fn normalize_queue_sizing(
        &self,
        store: &JobStorage,
        log_fn: &dyn Fn(&str),
    ) -> Result<usize, StorageError> {
        let paths: Vec<String> = store
            .list_blobs_with_meta(QUEUE_PREFIX)
            .await?
            .into_iter()
            .map(|blob| blob.name)
            .filter(|name| name.ends_with(".json"))
            .collect();
        if paths.is_empty() {
            return Ok(paths.len());
        }
        let observed = self.observed_map(store).await?;
        let no_stored_size = Job::default().gpu_mem_gb;
        let mut corrected = Vec::new();
        for text in download_many(store, &paths).await?.into_iter().flatten() {
            let Ok(job) = Job::from_json(&text) else {
                continue;
            };
            let model = model_of(&job.command);
            if model.is_empty() {
                continue;
            }
            let desired = match observed.get(&model) {
                Some(peak) => *peak,
                None => no_stored_size,
            };
            if job.gpu_mem_gb == desired {
                continue;
            }
            if store
                .update_queued_gpu_mem(&job.job_id, desired)
                .await?
                .is_some()
            {
                corrected.push(job.job_id);
            }
        }
        if !corrected.is_empty() {
            log_fn(&format!(
                "sizing: normalized {} queue jobs \
                 (unmeasured->unsized / measured->peak); stale-agent clobber corrected",
                corrected.len()
            ));
        }
        Ok(corrected.len())
    }
}
