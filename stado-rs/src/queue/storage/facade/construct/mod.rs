//! Backend selection: every constructor that turns configuration into a
//! bound [`JobStorage`], plus the disaster-recovery mirror it attaches.

use std::sync::Arc;

use crate::capabilities::{RuntimeAdapter, RuntimeFacet, StorageAdapter};
use crate::config;
use crate::queue::{construct_backend, BackendLocator, LocalBackend, StorageError};

use super::JobStorage;

mod root;
pub(crate) use root::StoreRoot;

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
        Self::with_bucket_read_mode(
            config::bucket(),
            super::failover::ReadMode::PrimaryOnly,
            StoreRoot::Client,
        )
        .await
    }
    /// Like [`JobStorage::for_primary_reads`] with an explicit bucket.
    ///
    /// Host-health readers keep the registry bucket's historical spelling on
    /// GCS, but liveness observations still must not fail over to a
    /// disaster-recovery copy and turn an unread primary into live state.
    pub(crate) async fn with_bucket_primary_reads(bucket: &str) -> Result<Self, StorageError> {
        Self::with_bucket_read_mode(
            bucket,
            super::failover::ReadMode::PrimaryOnly,
            StoreRoot::Client,
        )
        .await
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
        Self::with_bucket_read_mode(
            config::bucket(),
            super::failover::ReadMode::PrimaryOnly,
            StoreRoot::Served,
        )
        .await
    }

    /// Like [`JobStorage::new`] but binds the "gcs" backend to an explicit
    /// bucket (Python `JobStorage(bucket)`). The "local" backend ignores the
    /// bucket for routing — it is rooted at `config::wc_local_storage_path()`
    /// — but keeps it as `bucket_name` like Python `JobStorage(bucket)`.
    pub async fn with_bucket(bucket: &str) -> Result<Self, StorageError> {
        Self::with_bucket_read_mode(
            bucket,
            super::failover::ReadMode::Failover,
            StoreRoot::Client,
        )
        .await
    }

    /// The store that `stado://<namespace>/<key>` objects are addressed in.
    ///
    /// Their storage path ([`crate::remote::object_store::ObjectRef::storage_path`])
    /// already names the namespace, `ecosystem/<namespace>/<key>`, so it is
    /// resolved from the top of the store, as the object API server resolves
    /// it. [`JobStorage::new`] on the host that serves the store is a queue
    /// client rooted in the served queue namespace; through it the release
    /// agent wrote stado 0.23.19 to
    /// `ecosystem/probierz/ecosystem/releases/stado/0.23.19/…`, read it back
    /// there, called the platform published, and every delivery found
    /// `stado://releases/stado/0.23.19/<platform>/release.tar.gz` absent. On a
    /// host whose store nobody serves the two roots are the same directory.
    pub(crate) async fn for_object_uris() -> Result<Self, StorageError> {
        Self::with_bucket_read_mode(
            config::bucket(),
            super::failover::ReadMode::Failover,
            StoreRoot::Served,
        )
        .await
    }

    async fn with_bucket_read_mode(
        bucket: &str,
        read_mode: super::failover::ReadMode,
        root: StoreRoot,
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
        // bucket_name for wire compatibility. A store on another S3 API
        // endpoint is only ever the bucket its own binding declares, in the
        // region and under the account it names.
        let configured_s3_bucket = config::wc_s3_bucket();
        let (endpoint_bucket, region, account) = if adapter.is_s3_compatible() {
            (
                config::wc_s3_compatible_bucket(),
                config::wc_s3_compatible_region(),
                config::wc_s3_compatible_account(),
            )
        } else if adapter == StorageAdapter::S3 && !configured_s3_bucket.is_empty() {
            (
                configured_s3_bucket,
                config::wc_s3_region(),
                config::wc_azure_storage_account(),
            )
        } else {
            (
                bucket,
                config::wc_s3_region(),
                config::wc_azure_storage_account(),
            )
        };
        let local_path = (adapter == StorageAdapter::Local)
            .then(|| LocalBackend::resolved_root(config::wc_local_storage_path()))
            .transpose()?
            .map(|path| root.queue_root(&path).to_string_lossy().into_owned());
        // The store this opening addresses, as the layout check keys it:
        // every locator the backend below is built from, and the object API
        // URL and namespace a `stado` backend reads from the configuration.
        let store = format!(
            "{} bucket={endpoint_bucket} account={account} container={} region={region} root={:?} path={} url={} namespace={} endpoint={}",
            variant.id,
            config::wc_azure_container(),
            local_path,
            config::wc_local_storage_path(),
            config::wc_stado_storage_url_configured(),
            config::wc_stado_storage_namespace(),
            config::wc_s3_compatible_endpoint(),
        );
        let backend = construct_backend(
            adapter,
            BackendLocator {
                bucket: endpoint_bucket,
                account,
                container: config::wc_azure_container(),
                region,
                // A local store is rooted at its resolved queue root; every
                // other adapter is handed the configured local path as is.
                path: match local_path.as_deref() {
                    Some(path) => path,
                    None => config::wc_local_storage_path(),
                },
                endpoint: config::wc_s3_compatible_endpoint(),
                addressing: config::wc_s3_compatible_addressing(),
                credential_role: config::wc_s3_compatible_credential_role(),
            },
        )
        .await?;

        let mut storage = Self::with_backend_and_bucket(backend, variant.id, bucket);
        storage.local_path = local_path.map(Arc::from);
        storage.ensure_layout_once(&store).await?;
        storage.with_configured_read_failover(read_mode).await
    }

    /// Attach the configured disaster-recovery mirror using the selected read
    /// authority, when the backup can hold a replica of this primary at all.
    ///
    /// This is the OTHER writer to the backup: `ReadFailoverBackend` copies
    /// every `upload_*` to the backup as it happens, so it does not need
    /// replication to be switched on and it is not stopped by switching
    /// replication off. Unchecked, it refills `~/.stado/local-backup` at GiB
    /// per minute after the coordinator's replication has been stopped,
    /// because a `stado` primary names objects by bare key and a directory
    /// stores the name it is handed, so every artifact a job publishes lands at
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
        self,
        read_mode: super::failover::ReadMode,
    ) -> Result<Self, StorageError> {
        self.with_read_failover(
            super::copy::Endpoint::configured_primary(),
            super::copy::Endpoint::configured_backup(),
            read_mode,
        )
        .await
    }

    pub(super) async fn with_read_failover(
        mut self,
        primary: super::copy::Endpoint,
        backup: Option<super::copy::Endpoint>,
        read_mode: super::failover::ReadMode,
    ) -> Result<Self, StorageError> {
        let Some(mut endpoint) = backup else {
            return Ok(self);
        };
        if let Some(refusal) = primary.cannot_replicate(&endpoint) {
            eprintln!(
                "[storage-replica] no disaster-recovery mirror for this store: {refusal} \
                 Nothing is written to the backup and reads do not fail over to it."
            );
            return Ok(self);
        }
        if endpoint.adapter() == Some(StorageAdapter::Local) {
            // The mirror keeps the primary's layout: a client rooted in the
            // served queue namespace mirrors into the same namespace of the
            // backup, where the server's own mirror writes those keys.
            let resolved = LocalBackend::resolved_root(&endpoint.path)?;
            let primary_namespaced = self.local_path.as_deref().is_some_and(|path| {
                std::path::Path::new(path).ends_with(StoreRoot::namespace_tail())
            });
            endpoint.path = if primary_namespaced {
                StoreRoot::namespaced(&resolved)
            } else {
                resolved
            }
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
