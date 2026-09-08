//! Azure Blob Storage backend using the provider REST API.
//!
//! Authentication uses managed identity first and then the scoped
//! `stado-azure` service-principal item in Skarbiec. Conditional creates and
//! writes use `If-None-Match` and `If-Match`; lost races surface as
//! [`StorageError::StorageConflict`]. Reads pin the observed ETag and retry a
//! bounded concurrent-write race. Listing preserves opaque continuation
//! markers and blob metadata.
//!
//! The REST API version is pinned so conditional headers, metadata, and
//! pagination remain a release-visible contract.
//!
//! The seams this module was written along are now its components: the
//! constructor, the authenticated sender and the wire values read back off a
//! response (`client`), the blob and List Blobs URLs every request is
//! addressed through (`uri`), the non-success answers lifted into one error
//! shape (`refusals`), and the `BlobBackend` surface with the reads, writes,
//! listing walk and List Blobs parse behind it (`objects`).
//!
//! [`StorageError::StorageConflict`]: crate::queue::StorageError::StorageConflict

use std::sync::Arc;

mod client;
mod objects;
mod refusals;
mod uri;

/// Pinned Blob Storage REST API version (see module docs). Shared with
/// the release-channel fetcher in [`crate::self_update`], which speaks the
/// same REST surface when the release tree lives in a blob container.
pub(crate) const X_MS_VERSION: &str = "2023-11-03";
/// OAuth scope for the client-credentials token request.
pub(crate) const STORAGE_SCOPE: &str = "https://storage.azure.com/.default";
/// Resource for IMDS / az-CLI token requests (same audience).
pub(crate) const STORAGE_RESOURCE: &str = "https://storage.azure.com";
/// Service ceiling for `maxresults` on List Blobs, and the bulk page size
/// used when a listing has no window to fill.
const LIST_MAX_RESULTS: usize = 5_000;

struct Inner {
    http: reqwest::Client,
    account: String,
    container: String,
    /// `https://{account}.blob.core.windows.net` in prod, loopback in tests.
    base_url: String,
    /// True in prod (token chain attached); false on loopback test mocks.
    auth: bool,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("account", &self.account)
            .field("container", &self.container)
            .field("base_url", &self.base_url)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

/// Azure Blob implementation of [`BlobBackend`]. Cheap to clone.
///
/// [`BlobBackend`]: crate::queue::BlobBackend
#[derive(Clone, Debug)]
pub struct AzureBlobBackend {
    inner: Arc<Inner>,
}
