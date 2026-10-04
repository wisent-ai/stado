//! The read-only inspection commands — `ls`, `stat`, `cat` — over the ONE
//! configured store, and the addressing the two listing commands share.

use crate::cli::storage::*;

pub(in crate::cli::storage) mod cat;
pub(in crate::cli::storage) mod ls;
pub(in crate::cli::storage) mod stat;

/// The store a CLI path argument is resolved in: the whole store for a
/// `stado://` product URI, whose key names its namespace, and the queue
/// client's store for a bare queue path. On the host that serves the store
/// the queue client is rooted in the served queue namespace, so a URI
/// resolved through it named `ecosystem/probierz/ecosystem/<namespace>/…`
/// and `stat` called a published release absent.
pub(in crate::cli::storage) async fn store_for(path: &str) -> Result<JobStorage, CmdError> {
    Ok(if path.starts_with("stado://") {
        JobStorage::for_object_uris().await?
    } else {
        JobStorage::new().await?
    })
}

/// Resolve a CLI path argument to the key the backend actually stores under.
///
/// Two addressing forms reach the commands that take a path: a `stado://` product
/// URI, and a bare queue path, which is already a backend key. Only the explicit
/// scheme is rewritten, so queue callers are untouched.
///
/// WHICH key a URI becomes is the backend's answer, not this module's. The first
/// repair here rewrote every URI into the qualified store path
/// `ecosystem/<namespace>/<key>`, which is right for a filesystem or a bucket and
/// wrong for the object API: that backend re-prefixes with its own namespace, so
/// the qualified path asks it for `ecosystem/<ns>/ecosystem/<ns>/<key>`. With the
/// fleet store bound to the object API, `stat` therefore reported `absent` for
/// objects the same store served over HTTP 200 — the same doubled address a writer
/// defect had already created 417 real objects at.
pub(in crate::cli::storage) fn backend_key(
    backend: &Arc<dyn BlobBackend>,
    path: &str,
) -> Result<String, CmdError> {
    if path.starts_with("stado://") {
        Ok(backend.blob_path(&crate::remote::object_store::ObjectRef::parse(path)?))
    } else {
        Ok(path.to_string())
    }
}

/// The same resolution for a listing prefix, which may name a whole namespace and
/// therefore carry no key at all.
pub(in crate::cli::storage) fn backend_prefix(
    backend: &Arc<dyn BlobBackend>,
    prefix: &str,
) -> Result<String, CmdError> {
    match prefix.strip_prefix("stado://") {
        Some(rest) => {
            let (namespace, key) = rest.split_once('/').unwrap_or((rest, ""));
            if RemoteObjectApi::release_authorized(namespace, key) {
                return Err(CmdError::usage(
                    "release-governed stado:// prefixes must be listed with `stado storage objects \
                     <namespace> <prefix>` so the exact publisher credential is used",
                ));
            }
            Ok(backend.blob_prefix(namespace, key)?)
        }
        None => Ok(prefix.to_string()),
    }
}
