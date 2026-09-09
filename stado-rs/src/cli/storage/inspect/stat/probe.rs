//! The existence probe the five states come from.

use crate::cli::storage::*;

/// Existence probe for one object.
///
/// Deliberately NOT `BlobBackend::exists`: a `bool` cannot carry the store's
/// own refusal, and this command's whole job is to tell `refused`,
/// `unavailable` and `unreachable` apart from `absent`. Every backend
/// propagates its refusal today — Azure's HEAD through `ensure_success`, GCS's
/// `get_object`, S3's `head_object` outside `is_not_found`, the object API's
/// `stat` — but a caller reading a boolean would still have to flatten all of
/// them into "not there", which is the shape that made the billing outage
/// unreadable. The filesystem backend really did flatten them, until `exists`
/// stopped answering `Path::is_file`: a directory it could not traverse read
/// exactly like an empty one, and the object write plane turned that into
/// `404 {"state":"absent"}` for an object sitting on disk.
///
/// `download_text_versioned` propagates [`crate::queue::StorageError`]
/// instead, and answers existence, size and version token in one round trip.
pub(in crate::cli::storage) async fn probe(backend: &Arc<dyn BlobBackend>, path: &str) -> Presence {
    match backend.download_text_versioned(path).await {
        Ok(Some(found)) => Presence::Present {
            size: found.content.len(),
            version: Some(found.version),
            detail: None,
        },
        Ok(None) => Presence::Absent,
        // A non-UTF-8 body (a collected artifact) fails the versioned TEXT
        // read without the store being unreachable, so re-probe with the
        // binary read before calling it a transport failure. On a genuinely
        // unreachable store this second probe fails too and costs one
        // request.
        Err(err) => match backend.download_bytes(path).await {
            Ok(Some(bytes)) => Presence::Present {
                size: bytes.len(),
                version: None,
                detail: Some(format!("no version token: {err}")),
            },
            Ok(None) => Presence::Absent,
            Err(_) => unanswered_for_error(&err),
        },
    }
}
