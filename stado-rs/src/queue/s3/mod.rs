//! The S3 API backend, through the AWS SDK: Amazon S3 itself (`s3`), and any
//! other endpoint that speaks the S3 API, named by URL (`s3-compatible`).
//!
//! Conditional creates and compare-and-swap writes use native `If-None-Match`
//! and `If-Match` support. Amazon S3 documents both on PutObject. Any other
//! endpoint must prove it before the first write that relies on them: one
//! that ignored the headers would answer a lost race with success, and two
//! holders would each believe they own the same lease. ETags are opaque
//! backend version tokens. Missing objects and precondition failures are
//! classified from service status; transport and authorization failures
//! remain observable. Listing follows every provider continuation token and
//! retains metadata required by recovery.
//!
//! The seams this module was written along are now its components: the
//! constructor, the bucket every request is bound to and the two SDK answers
//! decoded back into crate values (`client`), the `CopySource` a copy-in-place
//! is addressed by (`uri`), the failed SDK calls classified and lifted into
//! one error shape (`refusals`), and the `BlobBackend` surface with the object
//! reads, the precondition-guarded writes and the paginated ListObjectsV2
//! walks behind it (`objects`).

use std::sync::Arc;

mod client;
mod compatible;
mod objects;
mod refusals;
mod uri;
pub mod vendor;

pub use compatible::S3CompatibleLocator;
pub use vendor::StorageVendor;

struct Inner {
    client: aws_sdk_s3::Client,
    bucket: String,
    conditional: Conditional,
}

/// Why this store's conditional writes are trusted.
enum Conditional {
    /// Amazon S3, which documents `If-None-Match` and `If-Match` on
    /// PutObject.
    Documented,
    /// Any other S3 API endpoint: `objects::probe` proves both once, before
    /// the first write that relies on them, and every later one reads the
    /// same outcome. `store` names the store in a refusal.
    Proved {
        store: String,
        outcome: tokio::sync::OnceCell<Result<(), String>>,
    },
}

/// S3 implementation of [`BlobBackend`]. Cheap to clone.
///
/// [`BlobBackend`]: crate::queue::BlobBackend
#[derive(Clone)]
pub struct S3Backend {
    inner: Arc<Inner>,
}
