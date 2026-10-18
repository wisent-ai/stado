//! The registry a resolver reads on each refresh: its authority's snapshot,
//! or, while that authority does not answer, a newer registry this host was
//! handed.
//!
//! A registry authority that stops answering would hold every resolver on the
//! last document it served if each one asked only that authority: no registry
//! naming another authority could reach a host while the old one is down. A
//! takeover writes the new registry, whose service directory
//! names the new authority under a higher generation, into each host's
//! last-known-good copy; the resolver adopts it here, and its next source is
//! the new authority. The generation is the fence: a later answer of the old
//! authority carries a lower one and is refused as a rollback.

use serde_json::Value;

use crate::monitor::host_silence;
use crate::service_resolution;

use crate::cli::resolver::directory::document::kept_last_good_document;

impl super::ResolverState {
    /// The document, its store version and its directory generation.
    pub(super) async fn read_registry(&self) -> Result<(Value, String, u64), String> {
        let source = self.source.read().await.clone();
        let unanswered = match source.fetch(host_silence::READER_RESOLVER).await {
            Ok(read) => return Ok(read),
            Err(error) => error.to_string(),
        };
        match self.handed_over().await {
            Ok(Some(read)) => {
                eprintln!(
                    "stado resolver: the registry authority did not answer ({unanswered}); \
                     adopting the newer registry this host was handed, directory generation {}",
                    read.2
                );
                Ok(read)
            }
            Ok(None) => Err(unanswered),
            Err(copy) => Err(format!(
                "{unanswered}; the last-known-good registry copy could not be read: {copy}"
            )),
        }
    }

    /// This host's last-known-good copy, when its directory generation is
    /// above the one this process serves; `None` otherwise.
    async fn handed_over(&self) -> Result<Option<(Value, String, u64)>, String> {
        let Some(copy) = kept_last_good_document()? else {
            return Ok(None);
        };
        let Some(directory) = service_resolution::directory(&copy)? else {
            return Ok(None);
        };
        let served = self.snapshot.read().await.directory_generation;
        Ok((directory.generation > served).then(|| {
            (
                copy,
                super::super::startup::LAST_KNOWN_GOOD_VERSION.to_string(),
                directory.generation,
            )
        }))
    }
}
