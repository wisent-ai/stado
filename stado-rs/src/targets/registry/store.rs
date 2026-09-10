use crate::targets::*;

// ---------------------------------------------------------------------------
// __init__.py — `_load_from_gcs` + source-aware loaders
// ---------------------------------------------------------------------------

/// Short-TTL in-process cache of the fetched registry (Python `_GCS_CACHE`).
pub(crate) static REGISTRY_CACHE: LazyLock<Mutex<Option<(Instant, Registry)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Store-relative `registry.json` download: `Ok(Some(text))` = fetched with
/// the store's generation token, `Ok(None)` = blob absent (Python
/// `blob.generation is None`), `Err(msg)` = the store could not be reached
/// at all.
///
/// The generation travels with the text because the last-known-good cache
/// records WHICH document it is holding ([`RegistryCacheMeta::generation`]):
/// a cache that can only say "some registry, once" cannot be compared with
/// the authority when the authority comes back.
pub type RegistryDownloader =
    Arc<dyn Fn() -> BoxFuture<'static, Result<Option<VersionedText>, String>> + Send + Sync>;

/// Test seam replacing the production download (loopback mocks).
static REGISTRY_DOWNLOADER: LazyLock<Mutex<Option<RegistryDownloader>>> =
    LazyLock::new(|| Mutex::new(None));

/// Install a downloader in place of the production fetch (tests only —
/// `#[doc(hidden)]`, not part of the crate's operational surface). Pair with
/// [`clear_registry_cache`] so a cached document never leaks across
/// tests, and serialize via `testutil::GLOBAL_STATE_LOCK`.
#[doc(hidden)]
pub fn set_registry_downloader_for_testing(downloader: Option<RegistryDownloader>) {
    *REGISTRY_DOWNLOADER
        .lock()
        .expect("registry downloader lock") = downloader;
}

/// Drop the cached registry so the next [`fetch_registry_remote`] call
/// re-downloads. Dashboard policy writes call this immediately after a
/// successful CAS; tests also use it to isolate downloader seams.
pub fn clear_registry_cache() {
    *REGISTRY_CACHE.lock().expect("registry cache lock") = None;
}

/// Read/write handle on the canonical registry document, backend-aware.
///
/// The registry is queue control-plane state, not product state. On the Stado
/// object backend it therefore always addresses
/// `stado://probierz/registry.json`, regardless of the product namespace in
/// the caller's ambient configuration. The configured token is still used,
/// so a caller without authority to read that namespace gets the real
/// permission failure rather than a document from a namespace it can read.
///
/// On every backend reads are pinned to the configured primary. A
/// disaster-recovery replica may lag the authority; silently accepting it as
/// the canonical document makes a healthy registry look rolled back while
/// [`registry_location`] continues to name the primary. Writes on direct
/// backends retain the configured mirror through
/// [`JobStorage::for_primary_reads`].
///
/// GCS keeps its historical dedicated object at [`GCS_REGISTRY_URI`]. Other
/// direct backends hold [`REGISTRY_BLOB`] at their root.
pub struct RegistryStore {
    backend: Arc<dyn BlobBackend>,
    blob: String,
    location: String,
}

impl RegistryStore {
    /// Bind to the one authoritative store that holds the canonical registry.
    pub async fn open() -> Result<Self, StorageError> {
        let adapter = crate::capabilities::storage_adapter(crate::config::wc_storage_backend());
        if adapter == Some(crate::capabilities::StorageAdapter::Gcs) {
            let uri = GCS_REGISTRY_URI
                .strip_prefix("gs://")
                .unwrap_or(GCS_REGISTRY_URI);
            let (bucket, blob) = uri.split_once('/').unwrap_or((uri, REGISTRY_BLOB));
            let backend = crate::queue::GcsBackend::new(bucket).await?;
            return Ok(Self {
                backend: Arc::new(backend),
                blob: blob.to_string(),
                location: GCS_REGISTRY_URI.to_string(),
            });
        }
        if adapter == Some(crate::capabilities::StorageAdapter::StadoObject) {
            let backend = crate::queue::StadoObjectBackend::new(
                crate::config::wc_stado_storage_url(),
                crate::config::QUEUE_OBJECT_NAMESPACE,
                crate::config::wc_stado_storage_token_file(),
                crate::config::wc_stado_storage_ca_file(),
            )?;
            return Ok(Self {
                backend: Arc::new(backend),
                blob: REGISTRY_BLOB.to_string(),
                location: registry_location(),
            });
        }
        let store = JobStorage::for_primary_reads().await?;
        Ok(Self {
            backend: Arc::clone(store.backend()),
            blob: REGISTRY_BLOB.to_string(),
            location: registry_location(),
        })
    }

    /// Operator-facing location of the object this handle addresses, in
    /// the spelling [`RegistryFetchError`] reports.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Registry text, or `None` when the object does not exist.
    pub async fn read_text(&self) -> Result<Option<String>, StorageError> {
        self.backend.download_text(&self.blob).await
    }

    /// Registry text plus the generation/ETag a compare-and-swap needs.
    pub async fn read_versioned(&self) -> Result<Option<VersionedText>, StorageError> {
        self.backend.download_text_versioned(&self.blob).await
    }

    /// Create the registry object; `false` when one already exists.
    pub async fn create_if_absent(&self, content: &str) -> Result<bool, StorageError> {
        self.backend
            .upload_text_if_absent(&self.blob, content)
            .await
    }

    /// Replace the registry iff its generation still matches; returns the
    /// new generation.
    pub async fn compare_and_swap(
        &self,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        self.backend
            .compare_and_swap_text(&self.blob, expected_version, content)
            .await
    }

    /// Create one object beside the registry document, never replacing one.
    ///
    /// "Beside" literally: the key is derived from the registry blob's own
    /// prefix on whichever store [`RegistryStore::open`] resolved, so the
    /// record of a registry mutation cannot end up in a different bucket from
    /// the document it is about — which on a GCS deployment, where the
    /// registry has its own bucket and [`crate::queue::JobStorage`] does not,
    /// is exactly what writing it through the queue store would do.
    ///
    /// Create-only, because an audit record that a later write can replace is
    /// not an audit record. Returns the full key and whether it was created.
    pub async fn write_beside(
        &self,
        relative: &str,
        content: &str,
    ) -> Result<(String, bool), StorageError> {
        let key = match self.blob.rsplit_once('/') {
            Some((prefix, _)) => format!("{prefix}/{relative}"),
            None => relative.to_string(),
        };
        let created = self.backend.upload_text_if_absent(&key, content).await?;
        Ok((key, created))
    }
}

/// Production download of the registry document through the store
/// `WC_STORAGE_BACKEND` selects.
///
/// On "gcs" this stays pinned to [`GCS_REGISTRY_URI`]'s own bucket via the
/// crate's [`crate::queue::GcsBackend`] (the GCS JSON API — never gsutil).
/// Every other backend reads [`REGISTRY_BLOB`] from
/// [`crate::queue::JobStorage`]: the same store the rest of the tick uses,
/// and the same one `dashboard/policy.rs` compare-and-swaps the registry
/// through. Python hardcodes GCS, so on an Azure-only deployment its
/// readers sit on a dead object while the dashboard edits the live one.
///
/// Python `_load_from_gcs` uses the GCS Python SDK for the same reason:
/// earlier this shelled out to `gsutil cat`, and on systems with a broken
/// gsutil install (cryptography/pyOpenSSL version mismatch breaking
/// `module 'OpenSSL.crypto' has no attribute 'sign'`) gsutil exited
/// non-zero and the agent crashed with 'hostname X not in registry' even
/// though the registry WAS in GCS — confirmed live on 2026-05-08, when the
/// workstation's gsutil broke after a pip upgrade and knocked the agent
/// offline. The GCS SDK was already a hard dependency; using it directly
/// removes the gsutil binary as a single point of failure.
async fn download_registry_blob() -> Result<Option<VersionedText>, String> {
    // One seam for both directions: [`RegistryStore`] resolves the same
    // object `cli/registry.rs::push` writes and `dashboard/policy.rs`
    // compare-and-swaps, so a reader can never sit on a dead object while
    // the writer edits a live one.
    let store = RegistryStore::open().await.map_err(|exc| exc.to_string())?;
    store.read_versioned().await.map_err(|exc| exc.to_string())
}

pub(crate) async fn download_registry() -> Result<Option<VersionedText>, String> {
    let downloader = REGISTRY_DOWNLOADER
        .lock()
        .expect("registry downloader lock")
        .clone();
    match downloader {
        Some(downloader) => downloader().await,
        None => download_registry_blob().await,
    }
}

/// Why the canonical registry could not be READ — as distinct from a
/// registry that WAS read and simply does not list a given entry.
///
/// The split is load-bearing. The coordinator's rogue-daemon kill switch
/// (`coordinator::run`) exits the process when a registry it successfully
/// read omits its entry, and must keep running when it could not read one
/// at all. Collapsing both into an empty registry is what took the fleet
/// down when the GCP billing account was closed: every GCS call started
/// answering `accountDisabled`, and the kill switch fired fleet-wide
/// against a registry nobody had touched.
#[derive(Debug, thiserror::Error)]
pub enum RegistryFetchError {
    /// The store refused or failed the read (auth, network, disabled
    /// billing account, ...). Says NOTHING about the registry's contents.
    #[error("registry store unreachable ({location}): {detail}")]
    Unreachable {
        /// Where the read was attempted, per `registry_location`.
        location: String,
        /// The underlying store error.
        detail: String,
    },
    /// The store answered, but holds no registry document. Just as
    /// non-authoritative about any single entry: a container nobody has
    /// seeded yet looks exactly like this, and the documented kill switch
    /// is "operator removed the ENTRY", never "operator deleted the whole
    /// registry".
    #[error("no registry document at {location}")]
    Absent {
        /// Where the read was attempted, per `registry_location`.
        location: String,
    },
    /// A document came back that is not a valid registry, so its contents
    /// cannot be trusted to revoke anything.
    #[error("invalid registry document at {location}: {source}")]
    Invalid {
        /// Where the document was read from, per `registry_location`.
        location: String,
        /// The parse failure.
        source: RegistryError,
    },
}

/// Operator-facing location of the canonical registry document.
///
/// The Stado object backend is namespace-addressed, so name the fixed queue
/// namespace rather than the caller's ambient product namespace. Other
/// backends keep their configured backend spelling; GCS retains its historical
/// dedicated URI.
pub fn registry_location() -> String {
    let backend = crate::config::wc_storage_backend();
    match crate::capabilities::storage_adapter(backend) {
        Some(crate::capabilities::StorageAdapter::Gcs) => GCS_REGISTRY_URI.to_string(),
        Some(crate::capabilities::StorageAdapter::StadoObject) => format!(
            "stado://{}/{}",
            crate::config::QUEUE_OBJECT_NAMESPACE,
            REGISTRY_BLOB
        ),
        _ => format!("{backend}:{REGISTRY_BLOB}"),
    }
}
