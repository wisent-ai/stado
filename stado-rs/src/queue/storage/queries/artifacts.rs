//! Per-job side objects: the launch script and the status/heartbeat blobs.

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

impl std::fmt::Display for StatusLeaf {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::primitives::word::write(self, formatter)
    }
}

impl StatusLeaf {
    /// Every leaf, so a key's segment is matched against the words these
    /// write rather than against a second spelling of them.
    const ALL: &'static [Self] = &[Self::Status, Self::Heartbeat];

    /// The leaf a key's last segment names, or `None` for any other key:
    /// any other key belongs to the job's output.
    pub fn parse(segment: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|leaf| leaf.to_string() == segment)
    }

    /// The object key of this leaf for `job_id`.
    pub fn path(self, job_id: &str) -> String {
        format!("status/{job_id}/{self}")
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

    // ---- status / heartbeat ----

    /// First whitespace-separated token of `status/{job_id}/status`, or
    /// `None` when absent.
    pub async fn read_status(&self, job_id: &str) -> Result<Option<String>, StorageError> {
        let Some(data) = self.download_text(&StatusLeaf::Status.path(job_id)).await? else {
            return Ok(None);
        };
        Ok(data.split_whitespace().next().map(str::to_string))
    }

    /// Remove both status blobs for a job.
    pub async fn cleanup_status(&self, job_id: &str) -> Result<(), StorageError> {
        for leaf in StatusLeaf::ALL {
            self.delete_blob(&leaf.path(job_id)).await?;
        }
        Ok(())
    }
}
