//! Google Cloud Storage backend using the JSON API and scoped token provider.
//!
//! Authentication accepts a GCP managed identity or the `stado-gcp` Skarbiec
//! service-account item; cloud CLI sessions and subprocess substitutes are not
//! credential sources. GCS generations provide create-if-absent and
//! compare-and-swap semantics. Authorization, transport, and non-precondition
//! provider failures remain observable to callers.
//!
//! The seams this module was written along are now its components: the
//! constructor, the scoped token and the authenticated sender every request
//! leaves through (`client`), the upload, object, media and listing URLs a
//! request is addressed by (`uri`), the non-success answers lifted into one
//! error shape (`refusals`), and the `BlobBackend` surface with the object
//! reads, the precondition-guarded writes and the paginated listing walks
//! behind it (`objects`).

use std::sync::Arc;

mod client;
mod objects;
mod refusals;
mod uri;

/// The RFC 3986 encoder the other provider clients in this crate address
/// their own URLs with: the S3 and Azure Blob backends, the Box HTTP layer,
/// the GCP compute and inventory clients, and the quota-SKU client.
pub(crate) use uri::percent_encode;

/// Read/write storage OAuth scope and JSON API base.
const STORAGE_SCOPE: &str = "https://www.googleapis.com/auth/devstorage.read_write";
const API_BASE: &str = "https://storage.googleapis.com";

struct Inner {
    client: reqwest::Client,
    bucket: String,
    auth: Arc<dyn gcp_auth::TokenProvider>,
}

/// GCS implementation of [`BlobBackend`]. Cheap to clone.
///
/// [`BlobBackend`]: crate::queue::BlobBackend
#[derive(Clone)]
pub struct GcsBackend {
    inner: Arc<Inner>,
}
