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
            .map(|path| root.queue_root(&path).to_string_lossy().into_owned());
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

/// Where a process roots a local store.
///
/// A store an object API serves keeps the fleet's queue under
/// `ecosystem/<QUEUE_OBJECT_NAMESPACE>/`, the keys every remote agent reads
/// and writes through that API. The server itself addresses the whole store
/// (`Served`); every other process on that host — the queue agent inside the
/// object API among them — is a queue client (`Client`) and roots its queue
/// in that namespace. Rooted at the store top, the mini's agent published its
/// capacity where no other host looks and claimed none of the fleet's jobs
/// (0b6008fe). A store no object API serves has no namespace directory and
/// stays rooted at its top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreRoot {
    Served,
    Client,
}

impl StoreRoot {
    fn namespace_tail() -> std::path::PathBuf {
        std::path::Path::new("ecosystem").join(config::QUEUE_OBJECT_NAMESPACE)
    }

    fn namespaced(root: &std::path::Path) -> std::path::PathBuf {
        root.join(Self::namespace_tail())
    }

    /// The directory this process's queue lives in, under a resolved root.
    pub(crate) fn queue_root(self, root: &std::path::Path) -> std::path::PathBuf {
        let served = Self::namespaced(root);
        if self == Self::Client && served.is_dir() {
            served
        } else {
            root.to_path_buf()
        }
    }

    /// The top of the store a queue client on the serving host is rooted
    /// under, when `queue_root` put it in the served queue namespace. Objects
    /// of other namespaces — a published release among them — live under
    /// that top, not under the client's queue directory.
    pub(crate) fn served_top(client_root: &std::path::Path) -> Option<std::path::PathBuf> {
        let tail = Self::namespace_tail();
        client_root
            .ends_with(&tail)
            .then(|| {
                client_root
                    .ancestors()
                    .nth(tail.components().count())
                    .map(std::path::Path::to_path_buf)
            })
            .flatten()
    }
}
