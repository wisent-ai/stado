//! Backend selection: every constructor that turns configuration into a
//! bound [`JobStorage`], plus the disaster-recovery mirror it attaches.

use std::sync::Arc;

use crate::capabilities::{RuntimeAdapter, RuntimeFacet, StorageAdapter};
use crate::config;
use crate::queue::{construct_backend, BackendLocator, LocalBackend, StorageError};

use super::JobStorage;

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
    /// Build a normal client store whose reads must remain on the configured
    /// primary while writes retain the configured disaster-recovery mirror.
    pub(crate) async fn for_primary_reads() -> Result<Self, StorageError> {
        Self::with_bucket_read_mode(config::bucket(), super::failover::ReadMode::PrimaryOnly).await
    }
    /// Like [`JobStorage::for_primary_reads`] with an explicit bucket.
    ///
    /// Host-health readers keep the registry bucket's historical spelling on
    /// GCS, but liveness observations still must not fail over to a
    /// disaster-recovery copy and turn an unread primary into live state.
    pub(crate) async fn with_bucket_primary_reads(bucket: &str) -> Result<Self, StorageError> {
        Self::with_bucket_read_mode(bucket, super::failover::ReadMode::PrimaryOnly).await
    }

    /// Build the authoritative backing store used by the Stado API server.
    ///
    /// The server must be configured with a direct primary. A `stado` primary
    /// names the API listener itself and cannot prove which direct store is its
    /// authority; the separately configured backup is disaster-recovery state,
    /// not an interchangeable primary. A valid direct primary is constructed
    /// without read failover so an authority read can never silently come from
    /// stale backup state.
    pub async fn for_server() -> Result<Self, StorageError> {
        if crate::capabilities::storage_adapter(config::wc_storage_backend())
            == Some(StorageAdapter::StadoObject)
        {
            return Err(StorageError::Other(
                "the Stado API server requires a direct authoritative primary; \
                 WC_STORAGE_BACKEND=stado names the server itself, and the backup \
                 backend is not a primary substitute"
                    .to_string(),
            ));
        }
        Self::for_primary_reads().await
    }

    /// Like [`JobStorage::new`] but binds the "gcs" backend to an explicit
    /// bucket (Python `JobStorage(bucket)`). The "local" backend ignores the
    /// bucket for routing — it is rooted at `config::wc_local_storage_path()`
    /// — but keeps it as `bucket_name` like Python `JobStorage(bucket)`.
    pub async fn with_bucket(bucket: &str) -> Result<Self, StorageError> {
        Self::with_bucket_read_mode(bucket, super::failover::ReadMode::Failover).await
    }

    async fn with_bucket_read_mode(
        bucket: &str,
        read_mode: super::failover::ReadMode,
    ) -> Result<Self, StorageError> {
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
        let local_path = (adapter == StorageAdapter::Local)
            .then(|| LocalBackend::resolved_root(config::wc_local_storage_path()))
            .transpose()?
            .map(|path| path.to_string_lossy().into_owned());
        let backend = construct_backend(
            adapter,
            BackendLocator {
                bucket: endpoint_bucket,
                account: config::wc_azure_storage_account(),
                container: config::wc_azure_container(),
                region: config::wc_s3_region(),
                path: local_path
                    .as_deref()
                    .unwrap_or_else(|| config::wc_local_storage_path()),
            },
        )
        .await?;

        let mut storage = Self::with_backend_and_bucket(backend, variant.id, bucket);
        storage.local_path = local_path.map(Arc::from);
        storage.ensure_layout().await?;
        storage.with_configured_read_failover(read_mode).await
    }

    /// Attach the configured disaster-recovery mirror using the selected read
    /// authority, when the backup can hold a replica of this primary at all.
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
    async fn with_configured_read_failover(
        mut self,
        read_mode: super::failover::ReadMode,
    ) -> Result<Self, StorageError> {
        let Some(mut endpoint) = super::copy::Endpoint::configured_backup() else {
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
        if endpoint.adapter() == Some(StorageAdapter::Local) {
            endpoint.path = LocalBackend::resolved_root(&endpoint.path)?
                .to_string_lossy()
                .into_owned();
        }
        let backup = endpoint.build().await?;
        self.backend = Arc::new(super::failover::ReadFailoverBackend::new(
            self.backend.clone(),
            backup,
            read_mode,
        ));
        self.backup_endpoint = Some(Arc::new(endpoint));
        Ok(self)
    }
}
