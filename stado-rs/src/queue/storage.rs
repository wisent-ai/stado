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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    /// Where the last bounded claimable-scan stopped in the priority index.
    ///
    /// Reachability, and nothing else. A budgeted scan that always restarted
    /// at the head of the index re-read the same head every poll and could
    /// never see a job sitting past the budget, which is the starvation the
    /// budget was supposed to bound rather than cause. Resuming from the last
    /// visited marker and wrapping at the end of the prefix means every queued
    /// job is reached within a bounded number of polls.
    ///
    /// In memory on purpose: it is a fairness hint, not a fact about the
    /// queue. Persisting it would add a document that has to be written on
    /// every poll and reconciled after every crash, to protect a value whose
    /// only failure mode is starting a scan one page early. Shared across
    /// clones because the clones are one worker's handle on one store.
    scan_cursor: Arc<std::sync::Mutex<String>>,
}

const TRANSITION_PREFIX: &str = "job-transitions";
const TRANSITION_SCHEMA: &str = "stado.job-transition.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobTransition {
    schema: String,
    transition_id: String,
    owner: String,
    job_id: String,
    from_prefix: String,
    to_prefix: String,
    source_version: String,
    source_digest: String,
    destination_version: Option<String>,
    state: String,
    created_at: String,
    destination_job: Job,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn transition_path(job_id: &str) -> String {
    format!("{TRANSITION_PREFIX}/{}.json", sha256_hex(job_id.as_bytes()))
}

fn prefix_state(prefix: &str) -> &str {
    if prefix == "queue" {
        crate::models::job_state::QUEUED
    } else {
        prefix
    }
}

const TRANSITION_FENCE_PREFIX: &str = "transitioning:";
const TRANSITION_CLEANED_PREFIX: &str = "transition-cleaned:";

fn transition_fence_state(transition_id: &str) -> String {
    format!("{TRANSITION_FENCE_PREFIX}{transition_id}")
}
fn transition_cleaned_state(transition_id: &str) -> String {
    format!("{TRANSITION_CLEANED_PREFIX}{transition_id}")
}

pub(crate) fn is_transition_sentinel_state(state: &str) -> bool {
    state.starts_with(TRANSITION_FENCE_PREFIX) || state.starts_with(TRANSITION_CLEANED_PREFIX)
}

fn merge_transition_destination(
    current: &Job,
    requested: &Job,
    from_prefix: &str,
    to_prefix: &str,
) -> Result<Job, StorageError> {
    if crate::queue::submit::immutable_job_projection(current)
        != crate::queue::submit::immutable_job_projection(requested)
    {
        return Err(StorageError::StorageConflict(format!(
            "{} immutable submission identity changed",
            current.job_id
        )));
    }
    let mut destination = current.clone();
    destination.state = prefix_state(to_prefix).to_string();
    // The worker lease belongs to `running/` and to nothing else: a queued or
    // terminal document carrying an expiry would either fence a reaper that
    // has no worker to lose to, or leave a dead owner's stamp on a finished
    // job.
    destination.lease_expires_at = None;
    match to_prefix {
        "running" => {
            destination.started_at = requested.started_at.clone();
            destination.instance_ref = requested.instance_ref.clone();
            destination.lease_expires_at = requested.lease_expires_at.clone();
        }
        "queue" if from_prefix == "running" => {
            destination.started_at = requested.started_at.clone();
            destination.instance_ref = requested.instance_ref.clone();
            destination.restarts = destination.restarts.max(requested.restarts);
            destination.last_restart = requested.last_restart.clone();
            destination.error = requested.error.clone();
            destination.preempt_count = destination.preempt_count.max(requested.preempt_count);
            destination.yield_count = destination.yield_count.max(requested.yield_count);
            destination.assigned_to = requested.assigned_to.clone();
        }
        "completed" | "uploaded" | "cancelled" => {
            destination.completed_at = requested.completed_at.clone();
            destination.instance_ref = requested.instance_ref.clone();
            destination.error = requested.error.clone();
        }
        "failed" => {
            destination.failed_at = requested.failed_at.clone();
            destination.instance_ref = requested.instance_ref.clone();
            destination.error = requested.error.clone();
        }
        _ => {}
    }
    if crate::queue::runs::TERMINAL_PREFIXES.contains(&to_prefix) {
        destination.peak_vram_gb = destination.peak_vram_gb.max(requested.peak_vram_gb);
        destination.peak_vram_per_gpu |= requested.peak_vram_per_gpu;
        for artifact in &requested.artifact_paths {
            if !destination.artifact_paths.contains(artifact) {
                destination.artifact_paths.push(artifact.clone());
            }
        }
    }
    Ok(destination)
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
            scan_cursor: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    /// Where the last bounded claimable-scan stopped in the priority index.
    ///
    /// A poisoned lock is not worth failing a queue poll over: the cursor is a
    /// fairness hint, so losing it costs one scan that starts at the head.
    pub(crate) fn scan_cursor(&self) -> String {
        self.scan_cursor
            .lock()
            .map(|cursor| cursor.clone())
            .unwrap_or_default()
    }

    /// Record where the next bounded claimable-scan should resume.
    pub(crate) fn set_scan_cursor(&self, cursor: String) {
        if let Ok(mut slot) = self.scan_cursor.lock() {
            *slot = cursor;
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

    /// One name-ascending page of `prefix`, strictly after `start_after`, at
    /// most `limit` names. See [`BlobBackend::list_page`] for the contract.
    ///
    /// The ordered walk of the priority index is built on this rather than on
    /// [`Self::list_paths`], which cannot answer "the next few names" without
    /// first materializing every name there is.
    pub(crate) async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        self.backend.list_page(prefix, start_after, limit).await
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
        // Index ordering rule (see `queue::listing` module docs): the marker
        // is written BEFORE the job blob is settled. Blob-then-marker leaves
        // an admitted job with no index entry if anything interrupts the
        // window — and since the index is the whole listing strategy for
        // `queue/`, that job is invisible to every scheduler while still
        // reporting `queued`. It used to self-heal only if the very same
        // caller retried admission under the same run id, which no other
        // participant can do on its behalf.
        //
        // Every queued job is indexed, not just the prioritized ones.
        // `priority_key` already sorts priority 0 correctly — it is the
        // largest inverted key, so those jobs land after all prioritized
        // work and FIFO among themselves — so this widens the index's
        // coverage without changing one byte of its name shape. The
        // listing walk can only be the ordered index if the index names
        // everything; while it named a subset, the unindexed remainder
        // needed a second, whole-prefix pass to be reachable at all.
        self.write_priority_marker(job).await?;
        let created = self
            .backend
            .upload_text_if_absent(&blob_path, &job.to_json())
            .await?;
        if created {
            let meta = Self::job_metadata(job);
            self.backend.set_metadata(&blob_path, &meta).await?;
        } else if !self.backend.exists(&blob_path).await? {
            // Lost the create to a generation that has since left `queue/`,
            // so the marker names nothing. Dropping it is an optimization,
            // not a correctness step: an orphan is skipped by the walk and
            // only costs scan budget.
            self.delete_priority_marker_for(job).await?;
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
        if current.state != crate::models::job_state::QUEUED {
            return Ok(());
        }
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
        self.write_priority_marker(&current).await?;
        if !self.backend.exists(&path).await? {
            self.delete_priority_marker_for(&current).await?;
        }
        Ok(())
    }

    async fn read_job_transition(
        &self,
        job_id: &str,
    ) -> Result<Option<(JobTransition, String)>, StorageError> {
        let Some(versioned) = self.read_text_versioned(&transition_path(job_id)).await? else {
            return Ok(None);
        };
        let transition: JobTransition = serde_json::from_str(&versioned.content)?;
        if transition.schema != TRANSITION_SCHEMA || transition.job_id != job_id {
            return Err(StorageError::Other(format!(
                "invalid durable transition record for {job_id}"
            )));
        }
        Ok(Some((transition, versioned.version)))
    }

    async fn set_transition_state(
        &self,
        transition_id: &str,
        job_id: &str,
        state: &str,
    ) -> Result<(), StorageError> {
        for _ in 0..16 {
            let Some((mut transition, version)) = self.read_job_transition(job_id).await? else {
                return Err(StorageError::Other(format!(
                    "durable transition {transition_id} disappeared"
                )));
            };
            if transition.transition_id != transition_id {
                return Err(StorageError::StorageConflict(format!(
                    "durable transition ownership changed for {job_id}"
                )));
            }
            if transition.state == state {
                return Ok(());
            }
            transition.state = state.to_string();
            match self
                .compare_and_swap_text(
                    &transition_path(job_id),
                    &version,
                    &serde_json::to_string_pretty(&transition)?,
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(StorageError::StorageConflict(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StorageError::StorageConflict(format!(
            "durable transition {transition_id} remained contended"
        )))
    }

    async fn retire_transition_source(
        &self,
        transition: &JobTransition,
    ) -> Result<(), StorageError> {
        let source_path = format!("{}/{}.json", transition.from_prefix, transition.job_id);
        let fence_state = transition_fence_state(&transition.transition_id);
        let cleaned_state = transition_cleaned_state(&transition.transition_id);
        for _ in 0..16 {
            let Some(versioned) = self.read_text_versioned(&source_path).await? else {
                return Ok(());
            };
            let mut source = Job::from_json(&versioned.content)?;
            if source.state == cleaned_state {
                return Ok(());
            }
            if source.state != fence_state {
                return Ok(());
            }
            if transition.from_prefix == "queue" {
                // The hot path: every job that leaves the queue passes here.
                // The fenced source still carries the `priority` and
                // `created_at` the marker name was built from, so the name is
                // computable and this is one delete. Walking the index for a
                // matching suffix instead would cost a full listing of the
                // queue per completed job now that every job is indexed.
                self.delete_priority_marker_for(&source).await?;
            }
            source.state = cleaned_state.clone();
            match self
                .compare_and_swap_text(&source_path, &versioned.version, &source.to_json())
                .await
            {
                Ok(_) => return Ok(()),
                Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StorageError::StorageConflict(format!(
            "source fence for transition {} remained contended",
            transition.transition_id
        )))
    }

    async fn finish_completed_transition(
        &self,
        transition: &JobTransition,
    ) -> Result<bool, StorageError> {
        let destination_path = format!("{}/{}.json", transition.to_prefix, transition.job_id);
        let destination = self
            .read_job(&transition.to_prefix, &transition.job_id)
            .await?
            .ok_or_else(|| {
                StorageError::StorageConflict(format!(
                    "completed transition {} has no destination",
                    transition.transition_id
                ))
            })?;
        if crate::queue::submit::immutable_job_projection(&destination)
            != crate::queue::submit::immutable_job_projection(&transition.destination_job)
            || destination.state != prefix_state(&transition.to_prefix)
        {
            return Err(StorageError::StorageConflict(format!(
                "{destination_path} does not match completed transition {}",
                transition.transition_id
            )));
        }
        if crate::queue::runs::TERMINAL_PREFIXES.contains(&transition.to_prefix.as_str()) {
            crate::queue::runs::record_terminal_outcome(self, &destination, &transition.to_prefix)
                .await?;
        }
        self.retire_transition_source(transition).await?;
        tombstone::on_transition(self, &destination, &transition.to_prefix).await;
        Ok(true)
    }

    /// Recover or finish the single durable lifecycle transition for a job.
    /// Recovery is ownership-independent: the persisted intent and source
    /// version are the fence, so any caller can complete an abandoned owner.
    pub async fn recover_job_transition(&self, job_id: &str) -> Result<bool, StorageError> {
        let Some((transition, _)) = self.read_job_transition(job_id).await? else {
            return Ok(false);
        };
        if transition.state == "aborted" {
            return Ok(false);
        }
        let source_path = format!("{}/{}.json", transition.from_prefix, job_id);
        let destination_path = format!("{}/{}.json", transition.to_prefix, job_id);
        let fence_state = transition_fence_state(&transition.transition_id);

        if transition.state == "completed" {
            return self.finish_completed_transition(&transition).await;
        }
        if transition.state != "prepared" {
            return Err(StorageError::Other(format!(
                "durable transition {} has invalid state {}",
                transition.transition_id, transition.state
            )));
        }

        match self.read_text_versioned(&source_path).await? {
            Some(versioned)
                if versioned.version == transition.source_version
                    && sha256_hex(versioned.content.as_bytes()) == transition.source_digest =>
            {
                let mut source = Job::from_json(&versioned.content)?;
                source.state = fence_state.clone();
                match self
                    .compare_and_swap_text(&source_path, &versioned.version, &source.to_json())
                    .await
                {
                    Ok(_) => {}
                    Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => {
                        return Ok(false)
                    }
                    Err(error) => return Err(error),
                }
            }
            Some(versioned) => {
                let source = Job::from_json(&versioned.content)?;
                if source.state != fence_state {
                    self.set_transition_state(&transition.transition_id, job_id, "aborted")
                        .await?;
                    return Ok(false);
                }
            }
            None => {
                let Some(destination) = self.read_job(&transition.to_prefix, job_id).await? else {
                    return Err(StorageError::StorageConflict(format!(
                        "transition {} lost both source and destination",
                        transition.transition_id
                    )));
                };
                if crate::queue::submit::immutable_job_projection(&destination)
                    != crate::queue::submit::immutable_job_projection(&transition.destination_job)
                    || destination.state != prefix_state(&transition.to_prefix)
                {
                    return Err(StorageError::StorageConflict(format!(
                        "{destination_path} does not match transition {}",
                        transition.transition_id
                    )));
                }
                self.set_transition_state(&transition.transition_id, job_id, "completed")
                    .await?;
                return self.finish_completed_transition(&transition).await;
            }
        }

        let mut installed_destination = None;
        for _ in 0..16 {
            let existing_versioned = self.read_text_versioned(&destination_path).await?;
            let Some(existing_versioned) = existing_versioned else {
                if self
                    .backend
                    .upload_text_if_absent(&destination_path, &transition.destination_job.to_json())
                    .await?
                {
                    installed_destination = Some(transition.destination_job.clone());
                    break;
                }
                continue;
            };
            let existing = Job::from_json(&existing_versioned.content)?;
            if existing.state.starts_with(TRANSITION_CLEANED_PREFIX) {
                match self
                    .compare_and_swap_text(
                        &destination_path,
                        &existing_versioned.version,
                        &transition.destination_job.to_json(),
                    )
                    .await
                {
                    Ok(_) => {
                        installed_destination = Some(transition.destination_job.clone());
                        break;
                    }
                    Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                    Err(error) => return Err(error),
                }
            }
            if crate::queue::submit::immutable_job_projection(&existing)
                != crate::queue::submit::immutable_job_projection(&transition.destination_job)
                || existing.state != prefix_state(&transition.to_prefix)
            {
                return Err(StorageError::StorageConflict(format!(
                    "{destination_path} conflicts with transition {}",
                    transition.transition_id
                )));
            }
            if transition.destination_version.as_deref()
                == Some(existing_versioned.version.as_str())
                && existing.to_json() != transition.destination_job.to_json()
            {
                match self
                    .compare_and_swap_text(
                        &destination_path,
                        &existing_versioned.version,
                        &transition.destination_job.to_json(),
                    )
                    .await
                {
                    Ok(_) => {
                        installed_destination = Some(transition.destination_job.clone());
                        break;
                    }
                    Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                    Err(error) => return Err(error),
                }
            } else {
                installed_destination = Some(existing);
                break;
            }
        }
        let destination = installed_destination.ok_or_else(|| {
            StorageError::StorageConflict(format!(
                "{destination_path} remained contended during transition {}",
                transition.transition_id
            ))
        })?;
        self.backend
            .set_metadata(&destination_path, &Self::job_metadata(&destination))
            .await?;
        if transition.to_prefix == "queue" {
            // Anything re-entering the queue is indexed, whatever its
            // priority: a requeued job with priority 0 that carried no marker
            // would be invisible to a listing that walks only the index.
            self.write_priority_marker(&destination).await?;
        }
        self.set_transition_state(&transition.transition_id, job_id, "completed")
            .await?;
        self.finish_completed_transition(&transition).await
    }

    async fn transition_job_if_version(
        &self,
        requested: &Job,
        from_prefix: &str,
        to_prefix: &str,
        expected_version: Option<&str>,
    ) -> Result<bool, StorageError> {
        for _ in 0..16 {
            self.recover_job_transition(&requested.job_id).await?;
            let source_path = format!("{from_prefix}/{}.json", requested.job_id);
            let Some(source_versioned) = self.read_text_versioned(&source_path).await? else {
                if let Some(existing) = self.read_job(to_prefix, &requested.job_id).await? {
                    if crate::queue::submit::immutable_job_projection(&existing)
                        == crate::queue::submit::immutable_job_projection(requested)
                    {
                        return Ok(true);
                    }
                }
                return Ok(false);
            };
            if expected_version.is_some_and(|expected| expected != source_versioned.version) {
                return Ok(false);
            }
            let current = Job::from_json(&source_versioned.content)?;
            if current.state != prefix_state(from_prefix) {
                self.recover_job_transition(&requested.job_id).await?;
                return Ok(false);
            }
            let destination_versioned = self
                .read_text_versioned(&format!("{to_prefix}/{}.json", requested.job_id))
                .await?;
            let (destination_basis, destination_version) = match destination_versioned {
                Some(existing_versioned) => {
                    let existing = Job::from_json(&existing_versioned.content)?;
                    if crate::queue::submit::immutable_job_projection(&existing)
                        != crate::queue::submit::immutable_job_projection(&current)
                    {
                        return Err(StorageError::StorageConflict(format!(
                            "{to_prefix}/{}.json belongs to different lifecycle data",
                            requested.job_id
                        )));
                    }
                    if existing.state.starts_with(TRANSITION_CLEANED_PREFIX) {
                        (requested.clone(), Some(existing_versioned.version))
                    } else if existing.state == prefix_state(to_prefix) {
                        (existing, Some(existing_versioned.version))
                    } else {
                        return Err(StorageError::StorageConflict(format!(
                            "{to_prefix}/{}.json is not reusable for lifecycle transition",
                            requested.job_id
                        )));
                    }
                }
                None => (requested.clone(), None),
            };
            let destination =
                merge_transition_destination(&current, &destination_basis, from_prefix, to_prefix)?;
            let transition_id = sha256_hex(
                format!(
                    "{}\0{}\0{}\0{}\0{}",
                    requested.job_id,
                    from_prefix,
                    to_prefix,
                    source_versioned.version,
                    destination.to_json()
                )
                .as_bytes(),
            );
            let candidate = JobTransition {
                schema: TRANSITION_SCHEMA.to_string(),
                transition_id,
                owner: uuid::Uuid::new_v4().simple().to_string(),
                job_id: requested.job_id.clone(),
                from_prefix: from_prefix.to_string(),
                to_prefix: to_prefix.to_string(),
                source_version: source_versioned.version.clone(),
                source_digest: sha256_hex(source_versioned.content.as_bytes()),
                destination_version,
                state: "prepared".into(),
                created_at: Utc::now().to_rfc3339(),
                destination_job: destination,
            };
            let path = transition_path(&requested.job_id);
            let body = serde_json::to_string_pretty(&candidate)?;
            let installed = match self.read_text_versioned(&path).await? {
                None => self.create_text_if_absent(&path, &body).await?,
                Some(active) => {
                    let existing: JobTransition = serde_json::from_str(&active.content)?;
                    if existing.state == "prepared" {
                        self.recover_job_transition(&requested.job_id).await?;
                        continue;
                    }
                    match self
                        .compare_and_swap_text(&path, &active.version, &body)
                        .await
                    {
                        Ok(_) => true,
                        Err(StorageError::StorageConflict(_)) => false,
                        Err(error) => return Err(error),
                    }
                }
            };
            if !installed {
                continue;
            }
            if self.recover_job_transition(&requested.job_id).await? {
                return Ok(true);
            }
        }
        Err(StorageError::StorageConflict(format!(
            "job {} remained contended during lifecycle transition",
            requested.job_id
        )))
    }

    /// Claim through a durable transition record. The running job is derived
    /// from the fresh versioned queue body; stale caller priority/assignment
    /// cannot overwrite a concurrent queued rewrite.
    ///
    /// The claimed running document carries a worker lease from the first
    /// instant it exists, so a claim that dies before its first heartbeat is
    /// still reaped on a stated expiry rather than on a guess about
    /// `started_at`.
    pub async fn claim_queued_job(&self, job: &Job) -> Result<bool, StorageError> {
        self.recover_job_transition(&job.job_id).await?;
        let queue_path = format!("queue/{}.json", job.job_id);
        let Some(versioned) = self.read_text_versioned(&queue_path).await? else {
            return Ok(false);
        };
        let current = Job::from_json(&versioned.content)?;
        if current.state != crate::models::job_state::QUEUED
            || current.assigned_to != job.assigned_to
            || crate::queue::submit::immutable_job_projection(&current)
                != crate::queue::submit::immutable_job_projection(job)
        {
            return Ok(false);
        }
        let cancellation = format!("cancellations/{}.json", job.job_id);
        let cancelled = format!("cancelled/{}.json", job.job_id);
        if self.backend.exists(&cancellation).await? || self.backend.exists(&cancelled).await? {
            return Ok(false);
        }
        let mut claimed = job.clone();
        claimed.lease_expires_at = Some(Self::lease_deadline());
        let moved = self
            .transition_job_if_version(&claimed, "queue", "running", Some(&versioned.version))
            .await?;
        if !moved {
            return Ok(false);
        }
        if self.backend.exists(&cancellation).await? || self.backend.exists(&cancelled).await? {
            return Ok(false);
        }
        Ok(true)
    }

    /// One worker-lease deadline from now, in the window the fleet already
    /// calls a dead heartbeat.
    fn lease_deadline() -> String {
        (Utc::now() + chrono::Duration::minutes(config::HEARTBEAT_STALE_MINUTES)).to_rfc3339()
    }

    /// Renew the running job's own lease by compare-and-swap on the running
    /// document.
    ///
    /// This is the fence, and it only works because it writes the SAME object
    /// the reaper pins: a renewal that lands while a reaper is mid-reap
    /// changes that object's version, so the reaper's version-pinned move
    /// fails and the live execution keeps its slot. A pulse written beside the
    /// job cannot do that, however recently it was read.
    ///
    /// `false` when the job is no longer a live running document (moved,
    /// deleted, or fenced mid-transition) — the caller has lost the job, not
    /// the write.
    pub async fn renew_running_lease(&self, job_id: &str) -> Result<bool, StorageError> {
        let path = format!("running/{job_id}.json");
        for _ in 0..3 {
            let Some(versioned) = self.read_text_versioned(&path).await? else {
                return Ok(false);
            };
            let mut job = Job::from_json(&versioned.content)?;
            if job.state != crate::models::job_state::RUNNING {
                return Ok(false);
            }
            job.lease_expires_at = Some(Self::lease_deadline());
            match self
                .compare_and_swap_text(&path, &versioned.version, &job.to_json())
                .await
            {
                Ok(_) => return Ok(true),
                Err(StorageError::StorageConflict(_)) => continue,
                Err(StorageError::NotFound(_)) => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        Err(StorageError::StorageConflict(format!(
            "running/{job_id}.json remained contended during lease renewal"
        )))
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
        let job = Job::from_json(&data)?;
        if is_transition_sentinel_state(&job.state) {
            return Ok(None);
        }
        Ok(Some(job))
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
        // Per the index ordering rule (see `queue::listing` module docs), the
        // new key is written BEFORE the superseded one is dropped. A priority
        // change re-keys the marker, and clean-then-write leaves the job with
        // NO marker if anything interrupts between the two steps — a
        // transient 5xx, or an operator's Ctrl-C on `job priority` — which
        // strands a queued job outside the index permanently. Write-then-
        // clean fails into a harmless duplicate under the old key instead.
        // `keep` stops the cleanup scan, which matches on job_id, from
        // deleting the marker just written.
        if let Some(job) = &updated {
            self.write_priority_marker(job).await?;
            let current = listing::marker_path(job);
            self.repair_priority_markers(job_id, Some(&current)).await?;
            if !self.backend.exists(&format!("queue/{job_id}.json")).await? {
                self.delete_priority_marker_for(job).await?;
                return Ok(None);
            }
        } else {
            // No current queued generation to index; anything still bearing
            // this id is an orphan.
            self.repair_priority_markers(job_id, None).await?;
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
        self.rewrite_queued_job(job_id, |job| job.assigned_to = assigned_to.to_string())
            .await
    }

    /// Delete the job blob; also drops the priority marker in `queue/`.
    ///
    /// The job is read before it is deleted so the marker can be removed by
    /// its exact name. A job already gone leaves a marker whose key cannot be
    /// computed, which is the orphan case the index walk repairs.
    pub async fn delete_job(&self, prefix: &str, job_id: &str) -> Result<(), StorageError> {
        let indexed = if prefix == "queue" {
            self.read_job(prefix, job_id).await?
        } else {
            None
        };
        self.delete_blob(&format!("{prefix}/{job_id}.json")).await?;
        if prefix == "queue" {
            match &indexed {
                Some(job) => self.delete_priority_marker_for(job).await?,
                None => self.repair_priority_markers(job_id, None).await?,
            }
        }
        Ok(())
    }

    /// Move through the recoverable transition record. No destination is
    /// created until the exact source generation has been fenced.
    pub async fn move_job(
        &self,
        job: &Job,
        from_prefix: &str,
        to_prefix: &str,
    ) -> Result<(), StorageError> {
        if self
            .transition_job_if_version(job, from_prefix, to_prefix, None)
            .await?
        {
            Ok(())
        } else {
            Err(StorageError::StorageConflict(format!(
                "{from_prefix}/{}.json changed before transition to {to_prefix}",
                job.job_id
            )))
        }
    }

    /// Version-pinned lifecycle move for decisions (lease expiry, liveness)
    /// made from a specific source read.
    pub async fn move_job_if_version(
        &self,
        job: &Job,
        from_prefix: &str,
        to_prefix: &str,
        expected_version: &str,
    ) -> Result<bool, StorageError> {
        self.transition_job_if_version(job, from_prefix, to_prefix, Some(expected_version))
            .await
    }

    // ---- delegates to queue/listing/ (priority markers + bulk fetch) ----

    /// Index entry for a queued job (`queue_priority/` marker).
    pub async fn write_priority_marker(&self, job: &Job) -> Result<(), StorageError> {
        listing::write_marker(self, job).await
    }

    /// Drop the marker this job names, in one delete.
    pub async fn delete_priority_marker_for(&self, job: &Job) -> Result<(), StorageError> {
        listing::delete_marker_for(self, job).await
    }

    /// Repair path: drop every marker naming `job_id` by walking the index,
    /// except `keep` when the caller has already written the current one.
    /// Only for the cases where the marker's key is not derivable from the
    /// job — an orphan, or a key superseded by a priority change.
    pub async fn repair_priority_markers(
        &self,
        job_id: &str,
        keep: Option<&str>,
    ) -> Result<(), StorageError> {
        listing::delete_markers_scanning(self, job_id, keep).await
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

    /// Every job id under `{prefix}/`, without downloading one document.
    /// See [`listing::list_job_ids`] for why a keep-list must use this and
    /// not [`Self::list_jobs`].
    pub async fn list_job_ids(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        listing::list_job_ids(self, prefix).await
    }

    /// Priority markers first, then oldest-first, counting only jobs the
    /// caller's own admission rule accepts. See [`listing::JobScan`] for why
    /// the window and the scanning cost are separate quantities.
    pub async fn list_claimable_jobs(
        &self,
        prefix: &str,
        scan: &listing::JobScan<'_>,
    ) -> Result<Vec<Job>, StorageError> {
        listing::list_claimable(self, prefix, scan).await
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
