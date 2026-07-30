//! JobStorage facade: backend selection + job-level operations.
//!
//! Port of `stado/queue/storage.py::JobStorage`. The Python class routes
//! through `_blob_backend` (Azure/local) or the GCS SDK; here every
//! operation goes through a single `Arc<dyn BlobBackend>` — see the module
//! docs in `queue/mod.rs` for the `_azure_backend` divergence note.
//!
//! All four Python backends are wired: "local" / "gcs" / "azure" / "s3".

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;

use crate::capabilities::{RuntimeAdapter, RuntimeFacet, StorageAdapter};
use crate::config;
use crate::models::Job;

#[cfg(test)]
use super::local_file::LocalBackend;
use super::{
    construct_backend, listing, tombstone, BackendLocator, BlobBackend, BlobInfo, StorageError,
    VersionedText,
};

const LAYOUT_PATH: &str = "system/storage-layout.json";

/// Job-level storage facade over a [`BlobBackend`]. Cheap to clone.
#[derive(Clone)]
pub struct JobStorage {
    backend: Arc<dyn BlobBackend>,
    backend_name: String,
    bucket_name: String,
}

fn validate_layout_document(raw: &str) -> Result<(), StorageError> {
    let document: serde_json::Value = serde_json::from_str(raw)?;
    let product = document.get("product").and_then(serde_json::Value::as_str);
    let version = document
        .get("schema_version")
        .and_then(serde_json::Value::as_u64);
    if product != Some("stado") {
        return Err(StorageError::Other(
            "storage layout marker belongs to another product".to_string(),
        ));
    }
    if version != Some(u64::from(super::STORAGE_LAYOUT_VERSION)) {
        return Err(StorageError::Other(format!(
            "unsupported storage layout schema_version {}; expected {}",
            version
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string()),
            super::STORAGE_LAYOUT_VERSION
        )));
    }
    Ok(())
}
impl JobStorage {
    /// Select the backend from `config::wc_storage_backend()`: "local" roots
    /// a [`LocalBackend`] at `config::wc_local_storage_path()`, "gcs"
    /// (default) binds a [`GcsBackend`] to `config::bucket()`, "azure" builds
    /// an [`AzureBlobBackend`] from `config::wc_azure_storage_account()` /
    /// `config::wc_azure_container()`, "s3" builds an [`S3Backend`] on
    /// `config::wc_s3_bucket()` (falling back to the passed bucket like
    /// Python `S3Backend(WC_S3_BUCKET or bucket_name, ...)`) in
    /// `config::wc_s3_region()`.
    pub async fn new() -> Result<Self, StorageError> {
        Self::with_bucket(config::bucket()).await
    }

    /// Like [`JobStorage::new`] but binds the "gcs" backend to an explicit
    /// bucket (Python `JobStorage(bucket)`). The "local" backend ignores the
    /// bucket for routing — it is rooted at `config::wc_local_storage_path()`
    /// — but keeps it as `bucket_name` like Python `JobStorage(bucket)`.
    pub async fn with_bucket(bucket: &str) -> Result<Self, StorageError> {
        let configured_backend = config::wc_storage_backend();
        let variant =
            crate::capabilities::constructible_variant(RuntimeFacet::Storage, configured_backend)
                .ok_or_else(|| {
                let choices = crate::capabilities::configurable_ids(RuntimeFacet::Storage)
                    .collect::<Vec<_>>()
                    .join("\", \"");
                StorageError::Other(format!(
                    "WC_STORAGE_BACKEND={configured_backend} is not supported (use \"{choices}\")"
                ))
            })?;
        let RuntimeAdapter::Storage(adapter) = variant.adapter else {
            return Err(StorageError::Other(format!(
                "storage variant {:?} has no storage adapter",
                variant.id
            )));
        };

        // Python: S3Backend(WC_S3_BUCKET or bucket_name, WC_S3_REGION).
        // The configured bucket wins, while the facade retains its caller's
        // bucket_name for wire compatibility.
        let configured_s3_bucket = config::wc_s3_bucket();
        let endpoint_bucket = if adapter == StorageAdapter::S3 && !configured_s3_bucket.is_empty() {
            configured_s3_bucket
        } else {
            bucket
        };
        let backend = construct_backend(
            adapter,
            BackendLocator {
                bucket: endpoint_bucket,
                account: config::wc_azure_storage_account(),
                container: config::wc_azure_container(),
                region: config::wc_s3_region(),
                path: config::wc_local_storage_path(),
            },
        )
        .await?;

        let storage = Self::with_backend_and_bucket(backend, variant.id, bucket);
        storage.ensure_layout().await?;
        storage.with_configured_read_failover().await
    }

    async fn with_configured_read_failover(mut self) -> Result<Self, StorageError> {
        let Some(endpoint) = super::copy::Endpoint::configured_backup() else {
            return Ok(self);
        };
        let primary = super::copy::Endpoint::configured_primary();
        if primary.describe() == endpoint.describe() {
            return Err(StorageError::Other(format!(
                "primary and backup resolve to the same store ({})",
                primary.describe()
            )));
        }
        let backup = endpoint.build().await?;
        self.backend = Arc::new(super::failover::ReadFailoverBackend::new(
            self.backend.clone(),
            backup,
        ));
        Ok(self)
    }

    async fn ensure_layout(&self) -> Result<(), StorageError> {
        if let Some(raw) = self.backend.download_text(LAYOUT_PATH).await? {
            return validate_layout_document(&raw);
        }
        let body = serde_json::to_string(&serde_json::json!({
            "product": "stado",
            "schema_version": super::STORAGE_LAYOUT_VERSION,
        }))?;
        if self
            .backend
            .upload_text_if_absent(LAYOUT_PATH, &body)
            .await?
        {
            return Ok(());
        }
        let raw = self
            .backend
            .download_text(LAYOUT_PATH)
            .await?
            .ok_or_else(|| {
                StorageError::Other(
                    "storage layout marker disappeared after concurrent creation".to_string(),
                )
            })?;
        validate_layout_document(&raw)
    }

    /// Bind the facade to an explicit backend (tests, custom deployments).
    /// `bucket_name` is left empty — Python `JobStorage` always has a real
    /// bucket, but custom-backend consumers (unit tests) never read it; the
    /// submit fallback in `queue::submit` resolves `config::bucket()` for
    /// the empty case.
    pub fn with_backend(backend: Arc<dyn BlobBackend>, backend_name: impl Into<String>) -> Self {
        Self::with_backend_and_bucket(backend, backend_name, "")
    }

    /// [`JobStorage::with_backend`] with an explicit bucket name.
    pub fn with_backend_and_bucket(
        backend: Arc<dyn BlobBackend>,
        backend_name: impl Into<String>,
        bucket_name: impl Into<String>,
    ) -> Self {
        Self {
            backend,
            backend_name: backend_name.into(),
            bucket_name: bucket_name.into(),
        }
    }

    /// Configured storage backend name ("gcs" / "local").
    pub fn backend_name(&self) -> &str {
        &self.backend_name
    }

    /// The bucket this facade was bound to (Python `store.bucket_name`).
    pub fn bucket_name(&self) -> &str {
        &self.bucket_name
    }

    /// The backend handle, for consumers that iterate [`BlobInfo`] (which
    /// deliberately carries no bound download/delete closures).
    pub fn backend(&self) -> &Arc<dyn BlobBackend> {
        &self.backend
    }

    // ---- thin blob delegates (Python `_upload_text` etc.) ----

    /// Unconditional overwrite of a text blob.
    pub async fn upload_text(&self, blob_path: &str, content: &str) -> Result<(), StorageError> {
        self.backend.upload_text(blob_path, content).await
    }

    /// Unconditional overwrite of a binary blob (box artifact collection;
    /// Python `blob.upload_from_string(bytes)`).
    pub async fn upload_bytes(&self, blob_path: &str, content: &[u8]) -> Result<(), StorageError> {
        self.backend.upload_bytes(blob_path, content).await
    }

    /// Text content of a blob, or `None` when absent.
    pub async fn download_text(&self, blob_path: &str) -> Result<Option<String>, StorageError> {
        self.backend.download_text(blob_path).await
    }

    /// Raw bytes of a blob, or `None` when absent. Python `read_bytes`.
    pub async fn read_bytes(&self, blob_path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.backend.download_bytes(blob_path).await
    }

    /// Download one blob to `filename`; `false` when it is absent.
    pub async fn download_blob(
        &self,
        blob_path: &str,
        filename: &Path,
    ) -> Result<bool, StorageError> {
        self.backend.download_to_filename(blob_path, filename).await
    }

    /// Atomically create a text blob; `false` if it already exists.
    pub async fn create_text_if_absent(
        &self,
        blob_path: &str,
        content: &str,
    ) -> Result<bool, StorageError> {
        self.backend.upload_text_if_absent(blob_path, content).await
    }

    /// Read text together with the backend generation/ETag used for CAS.
    pub async fn read_text_versioned(
        &self,
        blob_path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        self.backend.download_text_versioned(blob_path).await
    }

    /// Replace text iff the version matches; returns the new version.
    ///
    /// Python raises `ValueError` for an empty `expected_version` and
    /// `StorageConflict` when the race is lost.
    pub async fn compare_and_swap_text(
        &self,
        blob_path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        if expected_version.is_empty() {
            return Err(StorageError::Other(
                "expected_version is required for compare-and-swap".into(),
            ));
        }
        self.backend
            .compare_and_swap_text(blob_path, expected_version, content)
            .await
    }

    /// Atomically upload a local file; `false` if the blob exists.
    pub async fn upload_file_if_absent(
        &self,
        blob_path: &str,
        filename: &Path,
    ) -> Result<bool, StorageError> {
        self.backend
            .upload_file_if_absent(blob_path, filename)
            .await
    }

    /// Delete a blob. Idempotent. Python `_delete_blob`.
    pub async fn delete_blob(&self, blob_path: &str) -> Result<(), StorageError> {
        self.backend.delete(blob_path).await
    }

    /// Blob names under `prefix`; `oldest_first > 0` bounds the listing to
    /// the N oldest by creation time. Python `_list_paths`.
    pub async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        self.backend.list_paths(prefix, oldest_first).await
    }

    /// (name, updated, metadata) for every blob under prefix, so callers
    /// can filter on metadata before downloading the full body.
    pub async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        self.backend.list_blobs_with_meta(prefix).await
    }

    // ---- job operations ----

    /// Write the job JSON to `{prefix}/{job_id}.json`, stamp filter metadata
    /// (`gpu_mem_gb`, `priority`, `gpu_type`, provider routing), and — for
    /// priority>0 jobs in `queue/` — write the `queue_priority/` index marker.
    pub async fn write_job(&self, prefix: &str, job: &Job) -> Result<(), StorageError> {
        let blob_path = format!("{}/{}.json", prefix, job.job_id);
        self.backend.upload_text(&blob_path, &job.to_json()).await?;
        let meta = Self::job_metadata(job);
        self.backend.set_metadata(&blob_path, &meta).await?;
        if prefix == "queue" && job.priority > 0 {
            self.write_priority_marker(job).await?;
        }
        Ok(())
    }

    /// Atomically claim a queued job by creating its `running/` record.
    /// Exactly one agent can win the create-if-absent race. A concurrent
    /// cancellation marker fences the winner before workload execution.
    pub async fn claim_queued_job(&self, job: &Job) -> Result<bool, StorageError> {
        let running_path = format!("running/{}.json", job.job_id);
        if !self
            .backend
            .upload_text_if_absent(&running_path, &job.to_json())
            .await?
        {
            return Ok(false);
        }
        self.backend
            .set_metadata(&running_path, &Self::job_metadata(job))
            .await?;

        let cancellation = format!("cancellations/{}.json", job.job_id);
        let cancelled = format!("cancelled/{}.json", job.job_id);
        if self.backend.download_text(&cancellation).await?.is_some()
            || self.backend.download_text(&cancelled).await?.is_some()
        {
            self.backend.delete(&running_path).await?;
            return Ok(false);
        }

        self.backend
            .delete(&format!("queue/{}.json", job.job_id))
            .await?;
        self.delete_priority_marker(&job.job_id).await?;
        Ok(true)
    }
    pub async fn refresh_job_metadata(&self, prefix: &str, job: &Job) -> Result<(), StorageError> {
        let blob_path = format!("{}/{}.json", prefix, job.job_id);
        self.backend
            .set_metadata(&blob_path, &Self::job_metadata(job))
            .await
    }

    fn job_metadata(job: &Job) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("gpu_mem_gb".to_string(), job.gpu_mem_gb.to_string()),
            ("priority".to_string(), job.priority.to_string()),
            ("gpu_type".to_string(), job.gpu_type.clone()),
            ("provider".to_string(), job.provider.clone()),
            (
                "pin_to_provider".to_string(),
                job.pin_to_provider.to_string(),
            ),
        ])
    }

    /// Read a job blob; `None` when absent. Corrupt JSON propagates as an
    /// error (the Python code strict-raises since the listing extraction).
    pub async fn read_job(&self, prefix: &str, job_id: &str) -> Result<Option<Job>, StorageError> {
        let Some(data) = self
            .backend
            .download_text(&format!("{prefix}/{job_id}.json"))
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(Job::from_json(&data)?))
    }

    /// Rewrite a queued job priority in-place (read-modify-write).
    /// `false` when the job blob does not exist.
    pub async fn update_priority(
        &self,
        job_id: &str,
        prefix: &str,
        new_priority: i64,
    ) -> Result<bool, StorageError> {
        let Some(mut job) = self.read_job(prefix, job_id).await? else {
            return Ok(false);
        };
        job.priority = new_priority;
        self.write_job(prefix, &job).await?;
        Ok(true)
    }

    /// Delete the job blob; also drops the priority marker in `queue/`.
    pub async fn delete_job(&self, prefix: &str, job_id: &str) -> Result<(), StorageError> {
        self.delete_blob(&format!("{prefix}/{job_id}.json")).await?;
        if prefix == "queue" {
            self.delete_priority_marker(job_id).await?;
        }
        Ok(())
    }

    /// Move a job between prefixes: write-then-delete. NOT atomic — a crash
    /// between the two can leave the job in both prefixes (readers tolerate
    /// duplicates) or neither; kept from Python, where the same window
    /// exists.
    ///
    /// After the move, [`tombstone::on_transition`] writes a
    /// fixed/failed_again marker when this terminates a re-submitted job.
    pub async fn move_job(
        &self,
        job: &Job,
        from_prefix: &str,
        to_prefix: &str,
    ) -> Result<(), StorageError> {
        self.write_job(to_prefix, job).await?;
        self.delete_blob(&format!("{from_prefix}/{}.json", job.job_id))
            .await?;
        if from_prefix == "queue" {
            self.delete_priority_marker(&job.job_id).await?;
        }
        tombstone::on_transition(self, job, to_prefix).await;
        Ok(())
    }

    // ---- delegates to queue/listing/ (priority markers + bulk fetch) ----

    /// Index entry for priority>0 jobs (`queue_priority/` marker).
    pub async fn write_priority_marker(&self, job: &Job) -> Result<(), StorageError> {
        listing::write_marker(self, job).await
    }

    /// Remove any priority marker(s) for this job_id.
    pub async fn delete_priority_marker(&self, job_id: &str) -> Result<(), StorageError> {
        listing::delete_marker(self, job_id).await
    }

    /// Parallel-fetch job JSONs under `{prefix}/`. Python
    /// `JobStorage.list_jobs`.
    pub async fn list_jobs(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<Job>, StorageError> {
        listing::list_jobs(self, prefix, oldest_first).await
    }

    /// Top-N highest-priority jobs via the `queue_priority/` index. Python
    /// `JobStorage.list_priority_jobs`.
    pub async fn list_priority_jobs(
        &self,
        prefix: &str,
        top_n: usize,
    ) -> Result<Vec<Job>, StorageError> {
        listing::list_top_n(self, prefix, top_n).await
    }

    /// Priority markers first, then FIFO oldest_first, deduped by job_id.
    /// Python `JobStorage.list_jobs_priority_first`.
    pub async fn list_jobs_priority_first(
        &self,
        prefix: &str,
        cap: usize,
    ) -> Result<Vec<Job>, StorageError> {
        listing::list_priority_first(self, prefix, cap).await
    }

    /// Priority-aware fitting jobs with metadata pre-filtering. Python
    /// `JobStorage.list_jobs_fitting` (`cap` defaults to 4000 in Python).
    pub async fn list_jobs_fitting(
        &self,
        prefix: &str,
        max_gpu_mem_gb: i64,
        cap: usize,
    ) -> Result<Vec<Job>, StorageError> {
        listing::list_fitting(self, prefix, max_gpu_mem_gb, cap).await
    }

    /// All jobs grouped by prefix. Python `JobStorage.list_all_jobs`.
    pub async fn list_all_jobs(&self) -> Result<BTreeMap<String, Vec<Job>>, StorageError> {
        let mut result = BTreeMap::new();
        for prefix in [
            "queue",
            "running",
            "completed",
            "uploaded",
            "failed",
            "cancelled",
        ] {
            result.insert(prefix.to_string(), self.list_jobs(prefix, 0).await?);
        }
        Ok(result)
    }

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
        let Some(data) = self
            .download_text(&format!("status/{job_id}/status"))
            .await?
        else {
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
        let path = format!("status/{job_id}/heartbeat");
        let Some(updated) = self.backend.updated_at(&path).await? else {
            return Ok(false);
        };
        let age_minutes = (Utc::now() - updated).num_seconds() as f64 / 60.0;
        Ok(age_minutes > threshold_minutes as f64)
    }

    /// Remove both status blobs for a job.
    pub async fn cleanup_status(&self, job_id: &str) -> Result<(), StorageError> {
        for suffix in ["status", "heartbeat"] {
            self.delete_blob(&format!("status/{job_id}/{suffix}"))
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        let store = JobStorage::with_backend(Arc::new(backend), "local");
        (dir, store)
    }

    fn job(job_id: &str) -> Job {
        let mut job = Job::new(job_id, "echo hi");
        job.created_at = "2026-01-02T03:04:05+00:00".into();
        job.gpu_mem_gb = 24;
        job.gpu_type = "nvidia-l4".into();
        job
    }

    #[tokio::test]
    async fn write_read_job_round_trip_with_metadata() {
        let (_dir, store) = store();
        let j = job("j1");
        store.write_job("queue", &j).await.unwrap();
        let back = store.read_job("queue", "j1").await.unwrap().unwrap();
        assert_eq!(back.to_json(), j.to_json());
        assert!(store.read_job("queue", "missing").await.unwrap().is_none());

        // Metadata stamping filters blobs before download.
        let infos = store.list_blobs_with_meta("queue/").await.unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(
            infos[0].metadata,
            BTreeMap::from([
                ("gpu_mem_gb".into(), "24".into()),
                ("priority".into(), "0".into()),
                ("gpu_type".into(), "nvidia-l4".into()),
                ("provider".into(), "gcp".into()),
                ("pin_to_provider".into(), "false".into()),
            ])
        );
    }

    #[tokio::test]
    async fn priority_marker_key_format_matches_python() {
        let (_dir, store) = store();
        let mut j = job("j9");
        j.priority = 5;
        store.write_job("queue", &j).await.unwrap();
        // inv = 99999999 - 5 = 99999994; key = f"{inv:08d}-{created_at}-{jid}.json"
        let expected = "queue_priority/99999994-2026-01-02T03:04:05+00:00-j9.json";
        assert_eq!(
            store.list_paths("queue_priority/", 0).await.unwrap(),
            vec![expected]
        );
        assert_eq!(
            store.download_text(expected).await.unwrap().as_deref(),
            Some("{\"job_id\": \"j9\", \"priority\": 5}")
        );
        // priority 0 jobs get no marker.
        store.write_job("queue", &job("j0")).await.unwrap();
        assert_eq!(
            store.list_paths("queue_priority/", 0).await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn move_job_transfers_blob_and_drops_marker() {
        let (_dir, store) = store();
        let mut j = job("jm");
        j.priority = 7;
        store.write_job("queue", &j).await.unwrap();
        store.move_job(&j, "queue", "running").await.unwrap();
        assert!(store.read_job("queue", "jm").await.unwrap().is_none());
        assert!(store.read_job("running", "jm").await.unwrap().is_some());
        assert!(store
            .list_paths("queue_priority/", 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn delete_job_clears_blob_and_marker() {
        let (_dir, store) = store();
        let mut j = job("jd");
        j.priority = 3;
        store.write_job("queue", &j).await.unwrap();
        store.delete_job("queue", "jd").await.unwrap();
        assert!(store.read_job("queue", "jd").await.unwrap().is_none());
        assert!(store
            .list_paths("queue_priority/", 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn update_priority_rewrites_in_place() {
        let (_dir, store) = store();
        store.write_job("queue", &job("jp")).await.unwrap();
        assert!(store.update_priority("jp", "queue", 9).await.unwrap());
        assert_eq!(
            store
                .read_job("queue", "jp")
                .await
                .unwrap()
                .unwrap()
                .priority,
            9
        );
        assert!(!store.update_priority("missing", "queue", 1).await.unwrap());
    }

    #[tokio::test]
    async fn cas_delegates_and_empty_version_is_rejected() {
        let (_dir, store) = store();
        assert!(store.create_text_if_absent("state/x", "v1").await.unwrap());
        assert!(!store.create_text_if_absent("state/x", "v2").await.unwrap());
        let v1 = store.read_text_versioned("state/x").await.unwrap().unwrap();
        assert_eq!(v1.content, "v1");
        let v2 = store
            .compare_and_swap_text("state/x", &v1.version, "v2")
            .await
            .unwrap();
        assert_ne!(v2, v1.version);
        let err = store
            .compare_and_swap_text("state/x", "", "v3")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("expected_version is required"),
            "{err}"
        );
        let err = store
            .compare_and_swap_text("state/x", &v1.version, "v3")
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::StorageConflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn script_upload_read() {
        let (_dir, store) = store();
        assert_eq!(store.read_script("s1").await.unwrap(), "");
        store
            .upload_script("s1", "#!/bin/bash\necho go")
            .await
            .unwrap();
        assert_eq!(
            store.read_script("s1").await.unwrap(),
            "#!/bin/bash\necho go"
        );
    }

    #[tokio::test]
    async fn status_and_heartbeat_helpers() {
        let (_dir, store) = store();
        assert_eq!(store.read_status("h1").await.unwrap(), None);
        store
            .upload_text("status/h1/status", "running pid 123")
            .await
            .unwrap();
        assert_eq!(
            store.read_status("h1").await.unwrap().as_deref(),
            Some("running")
        );

        // Missing heartbeat is not stale; a fresh one is not stale either.
        assert!(!store.heartbeat_stale("h1", 15).await.unwrap());
        store.upload_text("status/h1/heartbeat", "x").await.unwrap();
        assert!(!store.heartbeat_stale("h1", 15).await.unwrap());
        // Negative threshold forces "stale" deterministically.
        assert!(store.heartbeat_stale("h1", -1).await.unwrap());

        store.cleanup_status("h1").await.unwrap();
        assert_eq!(store.read_status("h1").await.unwrap(), None);
        assert!(!store.heartbeat_stale("h1", -1).await.unwrap());
    }
}
