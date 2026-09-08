//! The `CopySource` value a copy-in-place is addressed by.
//!
//! Every other route names its object with a plain key the SDK escapes on the
//! wire; only the copy names a second object, and it names it as one string,
//! so the per-segment encoding boto3 applies to the dict form is written here
//! rather than inside the metadata write that needs it.

use crate::queue::gcs::percent_encode;

/// `CopySource` value for CopyObject: "bucket/key" with the key
/// percent-encoded except `/` separators (boto3's
/// `quote(key, safe='/~')` for the dict form of CopySource).
pub(super) fn copy_source(bucket: &str, key: &str) -> String {
    let encoded = key
        .split('/')
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/");
    format!("{bucket}/{encoded}")
}
