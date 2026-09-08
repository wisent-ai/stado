//! The existence probe the five states come from.

use crate::cli::storage::*;

/// Existence probe for one object.
///
/// Deliberately NOT `BlobBackend::exists`: the Azure implementation maps
/// every transport failure to `false` (Python parity), so a forbidden
/// container reads exactly like an empty one — the confusion that made the
/// billing outage unreadable. `download_text_versioned` propagates
/// [`crate::queue::StorageError`] instead, and answers existence, size and
/// version token in one round trip.
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
