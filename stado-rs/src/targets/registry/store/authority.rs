//! What a registry store read takes before the store itself, when the store
//! is an object API served by another host.

use super::RegistryStore;
use crate::queue::{StorageError, VersionedText};

impl RegistryStore {
    /// The registry this host's resolver holds, or else the registry
    /// authority's snapshot, when this store is an object API; `Ok(None)`
    /// sends the caller to the store. A read that could not be had is said
    /// on stderr with its own sentence, and the store is read: the one wait
    /// these reads exist to avoid is then visible as the store read's own
    /// `czekam` line. When the store is itself a service of the authority's
    /// host, which a read less than one refresh interval ago found silent,
    /// it is not asked either ([`StorageError::Held`] naming both), so the
    /// reader goes to its last-known-good copy at once instead of standing
    /// the relay's own wait on the same host.
    pub(super) async fn authority_snapshot(&self) -> Result<Option<VersionedText>, StorageError> {
        if !self.served_by_object_api {
            return Ok(None);
        }
        // Boxed: the authority's own read reaches a registry store again, so
        // the two futures name each other and need a pointer between them.
        match Box::pin(crate::cli::resolver::held_or_authority_snapshot()).await {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                let store = crate::config::wc_stado_storage_url().map_err(|cause| {
                    StorageError::Held(format!(
                        "the registry authority's snapshot could not be read ({error}), and this \
                         host's registry store cannot be named: {cause}"
                    ))
                })?;
                if let Some(shared) = crate::cli::resolver::store_on_silent_authority(&store) {
                    return Err(StorageError::Held(format!(
                        "the registry authority's snapshot could not be read ({error}), and {shared}"
                    )));
                }
                eprintln!(
                    "stado: the registry authority's snapshot could not be read ({error}); \
                     reading the registry store at {} instead",
                    self.location
                );
                Ok(None)
            }
        }
    }
}
