//! The automation facade itself: construction, the cross-prefix job read,
//! and the operations built on top of them.

use serde_json::{Map, Value};

use crate::config;
use crate::machine::contract::encoding::py_repr;
use crate::machine::contract::jobs::{normalize_job, JOB_PREFIXES};
use crate::machine::MachineError;
use crate::models::Job;
use crate::queue::JobStorage;

mod artifacts;
mod cancel;
mod logs;
mod submit;

/// The automation facade. Machine request submission is reserved with a
/// recoverable lease and delegated to the durable run manifest protocol.
pub struct MachineFacade {
    store: JobStorage,
    /// Where `stado://` objects (the uploaded source archive) are written and
    /// read back: the store top, where the object API serves them. On the host
    /// that serves the store, `store` is a queue client rooted in the served
    /// queue namespace, and an object written through it lands under
    /// `ecosystem/<namespace>/ecosystem/machine-inputs/…`, which no machine
    /// fetching `stado://machine-inputs/…` ever finds.
    objects: JobStorage,
    bucket: String,
}

impl MachineFacade {
    /// Facade over the configured storage backend (Python `MachineFacade()`
    /// → `JobStorage(BUCKET)`).
    pub async fn new() -> Result<Self, MachineError> {
        Ok(Self {
            store: JobStorage::new().await?,
            objects: JobStorage::for_object_uris().await?,
            bucket: config::bucket().to_string(),
        })
    }

    /// Facade over an explicit store that is already rooted at the store top,
    /// as the object API server's own store is; queue records and `stado://`
    /// objects both go through it. `bucket` remains the queue facade label
    /// passed to the submitter; product object locators are always
    /// provider-neutral `stado://` URIs.
    pub fn with_store(store: JobStorage, bucket: impl Into<String>) -> Self {
        Self {
            objects: store.clone(),
            store,
            bucket: bucket.into(),
        }
    }

    /// Read one job by id across every lifecycle prefix, stamping the
    /// prefix-derived state (Python `MachineFacade.lookup_job`).
    pub async fn lookup_job(&self, job_id: &str) -> Result<Job, MachineError> {
        let not_found = || {
            MachineError::new(
                "NOT_FOUND",
                format!("job {} was not found", py_repr(job_id)),
            )
        };
        if job_id.is_empty() || job_id.contains('/') || job_id.contains('\\') {
            return Err(not_found());
        }
        for prefix in JOB_PREFIXES {
            if let Some(mut job) = self.store.read_job(prefix, job_id).await? {
                job.state = if prefix == "queue" {
                    "queued".into()
                } else {
                    prefix.into()
                };
                return Ok(job);
            }
        }
        // A finished run is reaped: its terminal outcome — the whole job as it
        // ended — is retained in the run manifest, and the job's own documents
        // and log are deleted. A job that is gone for that reason is read back
        // from that outcome, not reported as one that never existed.
        if let Some(job) = self.reaped_job(job_id).await? {
            return Ok(job);
        }
        Err(not_found())
    }

    /// The job a reaped run retained for `job_id`, stamped with the terminal
    /// prefix it ended in, or `None` when no run names it
    /// ([`crate::queue::runs::retained_job`]).
    pub(crate) async fn reaped_job(&self, job_id: &str) -> Result<Option<Job>, MachineError> {
        Ok(crate::queue::runs::retained_job(&self.store, job_id).await?)
    }

    pub(crate) async fn observed_job(&self, job: &Job) -> Value {
        let mut value = normalize_job(job);
        value["provider_cleanup"] =
            match super::contract::cancellation::provider_cleanup(&self.store, job).await {
                Ok(observation) => observation.unwrap_or(Value::Null),
                Err(error) => serde_json::json!({
                    "job_id": job.job_id, "operation": "observe_instance_removal",
                    "removed": null, "error": error.to_string(),
                }),
            };
        value
    }

    /// Python `status`.
    pub async fn status(&self, job_id: &str) -> Result<Value, MachineError> {
        let job = self.lookup_job(job_id).await?;
        let mut out = Map::new();
        out.insert("job".into(), self.observed_job(&job).await);
        Ok(Value::Object(out))
    }

    /// `status`, held until the job ends. The change watch on the terminal
    /// prefixes is armed before the first read, so a job that ends between
    /// that read and the wait still wakes it; the answer comes when the
    /// job's terminal record is written, by this process or any other on the
    /// store's device. A failed watch is the error, never a silent re-read.
    pub async fn status_until_terminal(&self, job_id: &str) -> Result<Value, MachineError> {
        let mut watch = self
            .store
            .watch_prefixes(&crate::queue::runs::TERMINAL_PREFIXES)
            .map_err(|error| {
                MachineError::new(
                    "HOLD_UNAVAILABLE",
                    format!("until terminal cannot hold this read: {error}"),
                )
            })?;
        loop {
            let status = self.status(job_id).await?;
            if status.pointer("/job/terminal").and_then(Value::as_bool) != Some(false) {
                return Ok(status);
            }
            watch = tokio::task::spawn_blocking(move || {
                let mut armed = watch;
                armed.next().map(|()| armed)
            })
            .await
            .map_err(|error| {
                MachineError::new(
                    "HOLD_FAILED",
                    format!("the change watch holding {job_id} stopped: {error}"),
                )
            })?
            .map_err(|error| {
                MachineError::new(
                    "HOLD_FAILED",
                    format!("the change watch holding {job_id} failed: {error}"),
                )
            })?;
        }
    }
}
