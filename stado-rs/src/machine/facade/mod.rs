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
    bucket: String,
}

impl MachineFacade {
    /// Facade over the configured storage backend (Python `MachineFacade()`
    /// → `JobStorage(BUCKET)`).
    pub async fn new() -> Result<Self, MachineError> {
        Ok(Self::with_store(
            JobStorage::new().await?,
            config::bucket().to_string(),
        ))
    }

    /// Facade over an explicit store (tests, custom deployments). `bucket`
    /// remains the queue facade label passed to the submitter; product object
    /// locators are always provider-neutral `stado://` URIs.
    pub fn with_store(store: JobStorage, bucket: impl Into<String>) -> Self {
        Self {
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
        Err(not_found())
    }

    /// Python `status`.
    pub async fn status(&self, job_id: &str) -> Result<Value, MachineError> {
        let job = self.lookup_job(job_id).await?;
        let mut out = Map::new();
        out.insert("job".into(), normalize_job(&job));
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
