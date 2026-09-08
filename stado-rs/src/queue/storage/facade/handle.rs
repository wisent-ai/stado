//! Explicit handle construction and the facade's read-only accessors.

use std::sync::Arc;

use crate::queue::BlobBackend;

use super::JobStorage;

impl JobStorage {
    /// Bind the facade to an explicit backend (tests, custom deployments).
    /// `bucket_name` is left empty — Python `JobStorage` always has a real
    /// bucket, but custom-backend consumers (unit tests) never read it; the
    /// submit path in `queue::submit` resolves `config::bucket()` for
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
            local_path: None,
            backup_endpoint: None,
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

    /// The local root supplied when this facade constructed its backend.
    /// Custom backends do not claim a location they did not disclose.
    pub(crate) fn local_storage_path(&self) -> Option<&str> {
        self.local_path.as_deref()
    }

    /// The mirror actually constructed for this handle, not a later config read.
    pub(crate) fn backup_endpoint(&self) -> Option<&super::copy::Endpoint> {
        self.backup_endpoint.as_deref()
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
}
