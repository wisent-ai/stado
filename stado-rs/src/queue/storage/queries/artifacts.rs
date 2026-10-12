//! Per-job side objects: the launch script and the status/heartbeat blobs.

use chrono::Utc;

use crate::queue::StorageError;

use super::JobStorage;

/// The two objects a worker keeps under `status/<job_id>/`: its status word
/// and its heartbeat. Every other key there belongs to the job's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusLeaf {
    Status,
    Heartbeat,
}

impl StatusLeaf {
    /// The leaf a key's last segment names, or `None` for any other key.
    pub fn parse(segment: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(segment.to_string())).ok()
    }

    /// The object key of this leaf for `job_id`.
    pub fn path(self, job_id: &str) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(leaf)) => format!("status/{job_id}/{leaf}"),
            other => unreachable!("StatusLeaf serializes as a word, not {other:?}"),
        }
    }
}

impl JobStorage {
    // ---- scripts ----

    /// Upload an immutable launch script to the internal scheduler path and
    /// the provider-neutral product object exposed by `startup_script_uri`.
    pub async fn upload_script(&self, job_id: &str, content: &str) -> Result<(), StorageError> {
        self.upload_text(&format!("scripts/{job_id}.sh"), content)
            .await?;
        self.upload_text(&format!("ecosystem/jobs/{job_id}/startup-script"), content)
            .await
    }

    /// Read the job's launch script; "" when absent. Python
    /// `download_script`.
    pub async fn read_script(&self, job_id: &str) -> Result<String, StorageError> {
        Ok(self
            .download_text(&format!("scripts/{job_id}.sh"))
            .await?
            .unwrap_or_default())
    }

    // ---- status / heartbeat ----

    /// First whitespace-separated token of `status/{job_id}/status`, or
    /// `None` when absent.
    pub async fn read_status(&self, job_id: &str) -> Result<Option<String>, StorageError> {
        let Some(data) = self.download_text(&StatusLeaf::Status.path(job_id)).await? else {
            return Ok(None);
        };
        Ok(data.split_whitespace().next().map(str::to_string))
    }

    /// Whether the heartbeat blob at `status/{job_id}/heartbeat` is older
    /// than `threshold_minutes`. Missing heartbeat = not stale (Python
    /// parity).
    pub async fn heartbeat_stale(
        &self,
        job_id: &str,
        threshold_minutes: i64,
    ) -> Result<bool, StorageError> {
        let path = StatusLeaf::Heartbeat.path(job_id);
        let Some(updated) = self.backend.updated_at(&path).await? else {
            return Ok(false);
        };
        let age_minutes = (Utc::now() - updated).num_seconds() as f64 / 60.0;
        Ok(age_minutes > threshold_minutes as f64)
    }

    /// Remove both status blobs for a job.
    pub async fn cleanup_status(&self, job_id: &str) -> Result<(), StorageError> {
        for leaf in [StatusLeaf::Status, StatusLeaf::Heartbeat] {
            self.delete_blob(&leaf.path(job_id)).await?;
        }
        Ok(())
    }
}
