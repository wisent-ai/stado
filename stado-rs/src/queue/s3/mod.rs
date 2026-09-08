//! Amazon S3 backend using the AWS SDK.
//!
//! Conditional creates and compare-and-swap writes use native `If-None-Match`
//! and `If-Match` support. ETags are opaque backend version tokens. Missing
//! objects and precondition failures are classified from service status;
//! transport and authorization failures remain observable. Listing follows
//! every provider continuation token and retains metadata required by recovery.
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
mod objects;
mod refusals;
mod uri;

struct Inner {
    client: aws_sdk_s3::Client,
    bucket: String,
}

/// S3 implementation of [`BlobBackend`]. Cheap to clone.
///
/// [`BlobBackend`]: crate::queue::BlobBackend
#[derive(Clone)]
pub struct S3Backend {
    inner: Arc<Inner>,
}
