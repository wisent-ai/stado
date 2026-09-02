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
    /// Build the backing store used by the Stado API server itself.
    ///
    /// A client may select the `stado` adapter because it reaches this server,
    /// but the server cannot route its own queue and registry reads back through
    /// its listener before that listener has bound. In that topology the
    /// independently declared backup endpoint is the server's direct backing
    /// store. Any other primary remains unchanged.
    pub async fn for_server() -> Result<Self, StorageError> {
        let primary = super::copy::Endpoint::configured_primary();
        if primary.adapter() != Some(StorageAdapter::StadoObject) {
            return Self::new().await;
        }
        let endpoint = super::copy::Endpoint::configured_backup().ok_or_else(|| {
            StorageError::Other(
                "WC_STORAGE_BACKEND=stado cannot back the Stado API server itself; configure a direct WC_BACKUP_STORAGE_BACKEND".to_string(),
            )
        })?;
        if endpoint.adapter() == Some(StorageAdapter::StadoObject) {
            return Err(StorageError::Other(
                "the Stado API server backup backend must be direct, not stado".to_string(),
            ));
        }
        let backend = endpoint.build().await?;
        let storage =
            Self::with_backend_and_bucket(backend, endpoint.kind.clone(), endpoint.bucket.clone());
        storage.ensure_layout().await?;
        Ok(storage)
    }

    /// Like [`JobStorage::new`] but binds the "gcs" backend to an explicit
    /// bucket (Python `JobStorage(bucket)`). The "local" backend ignores the
    /// bucket for routing — it is rooted at `config::wc_local_storage_path()`
    /// — but keeps it as `bucket_name` like Python `JobStorage(bucket)`.
    pub async fn with_bucket(bucket: &str) -> Result<Self, StorageError> {
        // An unset backend with a configured local path is not a
        // misconfiguration to refuse; it is the local-only profile this machine
        // already runs. Erroring here turned every registry read into "the
        // service directory says nothing", which reads as an empty fleet rather
        // than as a client that never asked.
        let configured_backend = match config::wc_storage_backend() {
            "" if !config::wc_local_storage_path().is_empty() => "local",
            other => other,
        };
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

    /// Attach the read-failover mirror, when the configured backup can hold a
    /// replica of this primary at all.
    ///
    /// This is the OTHER writer to the backup, and until now the unchecked
    /// one: `ReadFailoverBackend` copies every `upload_*` to the backup as it
    /// happens, so it does not need replication to be switched on and it is not
    /// stopped by switching replication off. On charless-mac-mini it refilled
    /// `~/.stado/local-backup` at 2 GiB per minute — 48.29 GiB of proven
    /// duplicates deleted, back over 15 GiB seven minutes later — hours after
    /// the coordinator's replication had been stopped, because a `stado`
    /// primary names objects by bare key and a directory stores the name it is
    /// handed, so every artifact a job published landed at
    /// `local-backup/artifacts/...` where nothing looks for it.
    ///
    /// A pairing that cannot hold a replica gets NO mirror, and the reason is
    /// printed once. Not an error: erroring here would take down every stado
    /// process on a host whose configuration is merely worthless rather than
    /// dangerous — the agent, the coordinator and the object API server among
    /// them — and the store itself is fine. Read failover is dropped with it,
    /// which is honest, because a replica written at addresses nothing resolves
    /// could never have answered a read either.
    async fn with_configured_read_failover(mut self) -> Result<Self, StorageError> {
        let Some(endpoint) = super::copy::Endpoint::configured_backup() else {
            return Ok(self);
        };
        let primary = super::copy::Endpoint::configured_primary();
        if let Some(refusal) = primary.cannot_replicate(&endpoint) {
            eprintln!(
                "[storage-replica] no disaster-recovery mirror for this store: {refusal} \
                 Nothing is written to the backup and reads do not fail over to it."
            );
            return Ok(self);
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

    /// One `stado://releases/...` object off the public release channel —
    /// see [`BlobBackend::download_release`] for why the plain blob read is
    /// the wrong route for a cross-namespace software artifact.
    pub async fn download_release(&self, uri: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.backend.download_release(uri).await
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


    /// Create a queued job exactly once. Existing content is never overwritten;
    /// callers must read it back and verify its submission identity.
    pub async fn create_queued_job_if_absent(&self, job: &Job) -> Result<bool, StorageError> {
        let blob_path = format!("queue/{}.json", job.job_id);
        let created = self
            .backend
            .upload_text_if_absent(&blob_path, &job.to_json())
            .await?;
        if created {
            let meta = Self::job_metadata(job);
            self.backend.set_metadata(&blob_path, &meta).await?;
            if job.priority > 0 {
                self.write_priority_marker(job).await?;
            }
        }
        Ok(created)
    }

    /// Repair admission metadata only after a losing create has been read and
    /// validated against the durable planned job. A concurrent move turns this
    /// into a no-op and any marker written in that window is removed.
    pub async fn repair_queued_admission_metadata(
        &self,
        planned: &Job,
    ) -> Result<(), StorageError> {
        let path = format!("queue/{}.json", planned.job_id);
        let Some(versioned) = self.read_text_versioned(&path).await? else {
            return Ok(());
        };
        let current = Job::from_json(&versioned.content)?;
        if crate::queue::submit::immutable_job_projection(&current)
            != crate::queue::submit::immutable_job_projection(planned)
        {
            return Err(StorageError::StorageConflict(format!(
                "{path} does not match validated durable admission"
            )));
        }
        match self
            .backend
            .set_metadata(&path, &Self::job_metadata(&current))
            .await
        {
            Ok(()) => {}
            Err(StorageError::NotFound(_)) => return Ok(()),
            Err(error) => return Err(error),
        }
        if current.priority > 0 {
            self.write_priority_marker(&current).await?;
        }
        if !self.backend.exists(&path).await? {
            self.delete_priority_marker(&planned.job_id).await?;
        }
        Ok(())
    }

    /// Atomically claim the current queued generation. The running destination
    /// is create-only and the queued source is CAS-fenced, so a stale listing
    /// can neither recreate running nor erase a concurrent queue rewrite.
    pub async fn claim_queued_job(&self, job: &Job) -> Result<bool, StorageError> {
        let queue_path = format!("queue/{}.json", job.job_id);
        let Some(versioned) = self.read_text_versioned(&queue_path).await? else {
            return Ok(false);
        };
        let current = Job::from_json(&versioned.content)?;
        if current.state != crate::models::job_state::QUEUED
            || crate::queue::submit::immutable_job_projection(&current)
                != crate::queue::submit::immutable_job_projection(job)
        {
            return Ok(false);
        }
        for terminal in crate::queue::runs::TERMINAL_PREFIXES {
            if self
                .backend
                .exists(&format!("{terminal}/{}.json", job.job_id))
                .await?
            {
                return Ok(false);
            }
        }
        let cancellation = format!("cancellations/{}.json", job.job_id);
        let cancelled = format!("cancelled/{}.json", job.job_id);
        if self.backend.exists(&cancellation).await? || self.backend.exists(&cancelled).await? {
            return Ok(false);
        }
        let running_path = format!("running/{}.json", job.job_id);
        if !self
            .backend
            .upload_text_if_absent(&running_path, &job.to_json())
            .await?
        {
            return Ok(false);
        }
        if self.backend.exists(&cancellation).await? || self.backend.exists(&cancelled).await? {
            self.backend.delete(&running_path).await?;
            return Ok(false);
        }
        match self
            .compare_and_swap_text(&queue_path, &versioned.version, &job.to_json())
            .await
        {
            Ok(_) => {}
            Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => {
                self.backend.delete(&running_path).await?;
                return Ok(false);
            }
            Err(error) => {
                self.backend.delete(&running_path).await?;
                return Err(error);
            }
        }
        self.backend
            .set_metadata(&running_path, &Self::job_metadata(job))
            .await?;
        self.backend.delete(&queue_path).await?;
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

    async fn rewrite_queued_job<F>(
        &self,
        job_id: &str,
        mutate: F,
    ) -> Result<Option<Job>, StorageError>
    where
        F: Fn(&mut Job),
    {
        let path = format!("queue/{job_id}.json");
        for _ in 0..3 {
            let Some(versioned) = self.read_text_versioned(&path).await? else {
                return Ok(None);
            };
            let mut job = Job::from_json(&versioned.content)?;
            if job.state != crate::models::job_state::QUEUED {
                return Ok(None);
            }
            mutate(&mut job);
            match self
                .compare_and_swap_text(&path, &versioned.version, &job.to_json())
                .await
            {
                Ok(_) => {}
                Err(StorageError::StorageConflict(_)) => continue,
                Err(error) => return Err(error),
            }
            if !self.backend.exists(&path).await? {
                return Ok(None);
            }
            if let Err(error) = self
                .backend
                .set_metadata(&path, &Self::job_metadata(&job))
                .await
            {
                if matches!(&error, StorageError::NotFound(_)) {
                    return Ok(None);
                }
                return Err(error);
            }
            if !self.backend.exists(&path).await? {
                return Ok(None);
            }
            return Ok(Some(job));
        }
        Err(StorageError::StorageConflict(format!(
            "queue/{job_id}.json remained contended during rewrite"
        )))
    }

    /// CAS-update one current queued generation's priority and marker.
    pub async fn update_queued_priority(
        &self,
        job_id: &str,
        new_priority: i64,
    ) -> Result<Option<Job>, StorageError> {
        if !(0..=99_999_999).contains(&new_priority) {
            return Err(StorageError::Other(
                "job priority must be between 0 and 99999999".into(),
            ));
        }
        let updated = self
            .rewrite_queued_job(job_id, |job| job.priority = new_priority)
            .await?;
        self.delete_priority_marker(job_id).await?;
        if let Some(job) = &updated {
            if new_priority > 0 {
                self.write_priority_marker(job).await?;
            }
            if !self.backend.exists(&format!("queue/{job_id}.json")).await? {
                self.delete_priority_marker(job_id).await?;
                return Ok(None);
            }
        }
        Ok(updated)
    }

    /// CAS-update the measured queue sizing without recreating moved work.
    pub async fn update_queued_gpu_mem(
        &self,
        job_id: &str,
        gpu_mem_gb: i64,
    ) -> Result<Option<Job>, StorageError> {
        self.rewrite_queued_job(job_id, |job| job.gpu_mem_gb = gpu_mem_gb)
            .await
    }

    /// CAS-update the makespan assignment without recreating moved work.
    pub async fn update_queued_assignment(
        &self,
        job_id: &str,
        assigned_to: &str,
    ) -> Result<Option<Job>, StorageError> {
        self.rewrite_queued_job(job_id, |job| {
            job.assigned_to = assigned_to.to_string()
        })
        .await
    }

    /// Delete the job blob; also drops the priority marker in `queue/`.
    pub async fn delete_job(&self, prefix: &str, job_id: &str) -> Result<(), StorageError> {
        self.delete_blob(&format!("{prefix}/{job_id}.json")).await?;
        if prefix == "queue" {
            self.delete_priority_marker(job_id).await?;
        }
        Ok(())
    }

    /// Move one current source generation to a create-only destination.
    ///
    /// The source is read versioned before the destination is created, then
    /// CAS-rewritten to the destination state as a durable fence before the
    /// source path is deleted. A crash can leave two readable copies, but a
    /// stale caller can never create a destination after the source has moved.
    pub async fn move_job(
        &self,
        job: &Job,
        from_prefix: &str,
        to_prefix: &str,
    ) -> Result<(), StorageError> {
        let from = format!("{from_prefix}/{}.json", job.job_id);
        let to = format!("{to_prefix}/{}.json", job.job_id);
        let Some(versioned) = self.read_text_versioned(&from).await? else {
            if let Some(existing) = self.read_job(to_prefix, &job.job_id).await? {
                if serde_json::to_value(existing)? == serde_json::to_value(job)? {
                    return Ok(());
                }
            }
            return Err(StorageError::StorageConflict(format!(
                "{from} no longer exists; refusing stale move to {to}"
            )));
        };
        let current = Job::from_json(&versioned.content)?;
        let expected_source_state = if from_prefix == "queue" {
            crate::models::job_state::QUEUED
        } else {
            from_prefix
        };
        if current.state != expected_source_state && current.state != job.state {
            return Err(StorageError::StorageConflict(format!(
                "{from} is in state {}, not {expected_source_state}",
                current.state
            )));
        }
        if crate::queue::submit::immutable_job_projection(&current)
            != crate::queue::submit::immutable_job_projection(job)
        {
            return Err(StorageError::StorageConflict(format!(
                "{from} immutable submission identity changed"
            )));
        }
        let body = job.to_json();
        let created = self.backend.upload_text_if_absent(&to, &body).await?;
        if !created {
            let existing = self
                .read_job(to_prefix, &job.job_id)
                .await?
                .ok_or_else(|| {
                    StorageError::StorageConflict(format!(
                        "{to} won create-if-absent but is unreadable"
                    ))
                })?;
            if serde_json::to_value(existing)? != serde_json::to_value(job)? {
                return Err(StorageError::StorageConflict(format!(
                    "{to} already contains a different lifecycle outcome"
                )));
            }
        }
        match self
            .compare_and_swap_text(&from, &versioned.version, &body)
            .await
        {
            Ok(_) => {}
            Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => {
                if created {
                    self.backend.delete(&to).await?;
                }
                return Err(StorageError::StorageConflict(format!(
                    "{from} changed during move to {to}"
                )));
            }
            Err(error) => {
                if created {
                    self.backend.delete(&to).await?;
                }
                return Err(error);
            }
        }
        if crate::queue::runs::TERMINAL_PREFIXES.contains(&to_prefix) {
            crate::queue::runs::record_terminal_outcome(self, job, to_prefix).await?;
        }
        if created {
            self.backend
                .set_metadata(&to, &Self::job_metadata(job))
                .await?;
        }
        self.backend.delete(&from).await?;
        if from_prefix == "queue" {
            self.delete_priority_marker(&job.job_id).await?;
        }
        if to_prefix == "queue" {
            if created && job.priority > 0 {
                self.write_priority_marker(job).await?;
            }
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
