use std::sync::Arc;

use serde_json::Value;

use crate::monitor::host_silence;
use crate::service_resolution;
use crate::targets::RegistryStore;

use crate::cli::CmdError;

use crate::cli::resolver::directory::document::last_good_document;
use crate::cli::resolver::directory::read_local_snapshot;
use crate::cli::resolver::directory::source::current_target;
use crate::cli::resolver::directory::source::snapshot_source;
use crate::cli::resolver::directory::source::SnapshotSource;
use crate::cli::resolver::report::published::publish;
use crate::cli::resolver::report::published::PublishedState;

/// Everything `serve` must read before it can bind one port.
pub(super) struct Startup {
    pub(super) source: SnapshotSource,
    pub(super) document: Value,
    pub(super) store_version: String,
    pub(super) directory_generation: u64,
    pub(super) recovered: bool,
}

/// Every read `serve` needs before it binds, each failing with the
/// upstream's own words. A registry the local store or the last-known-good
/// copy already hands over is served at once when the authority's first
/// snapshot does not answer: the refresh loop keeps asking the authority,
/// and a resolver that bound nothing while that read stood held every
/// consumer on its host for as long as the authority stayed silent.
async fn load_startup(
    target: &str,
    local_store: Option<&Arc<RegistryStore>>,
) -> Result<Startup, String> {
    let (bootstrap, bootstrap_version, recovered) = match local_store {
        Some(local_store) => match read_local_snapshot(local_store).await {
            Ok((document, store_version, _)) => (document, store_version, false),
            Err(authority_error) => {
                let document = last_good_document().map_err(|cache_error| {
                    format!(
                        "registry authority failed ({authority_error}); recovery registry failed ({cache_error})"
                    )
                })?;
                eprintln!(
                    "stado resolver recovery: registry authority failed ({authority_error}); bootstrapping routing from the last-known-good registry"
                );
                (document, "last-known-good".to_string(), true)
            }
        },
        None => {
            let document = last_good_document()?;
            eprintln!(
                "stado resolver recovery: registry backend construction failed; bootstrapping routing from the last-known-good registry"
            );
            (document, "last-known-good".to_string(), true)
        }
    };
    let detected_target = current_target(&bootstrap)?;
    if detected_target != target {
        return Err(format!(
            "resolver target {target:?} does not match this host ({detected_target:?})"
        ));
    }
    let source = snapshot_source(local_store.cloned(), &bootstrap, target)?;
    let bootstrap_generation = || -> Result<u64, String> {
        Ok(service_resolution::directory(&bootstrap)?
            .ok_or_else(|| "the bootstrap registry has no service_directory".to_string())?
            .generation)
    };
    let (document, store_version, directory_generation, recovered) = if recovered {
        let generation = bootstrap_generation()?;
        eprintln!(
            "stado resolver recovery: serving the last-known-good generation {generation} immediately while authority retries continue"
        );
        (bootstrap, bootstrap_version, generation, true)
    } else {
        match source.fetch(host_silence::READER_RESOLVER).await {
            Ok((document, store_version, generation)) => {
                (document, store_version, generation, false)
            }
            Err(error) => {
                let generation = bootstrap_generation()?;
                eprintln!(
                    "stado resolver recovery: the registry authority's first snapshot did not answer ({error}); serving generation {generation} from the registry store immediately while authority retries continue"
                );
                (bootstrap, bootstrap_version, generation, true)
            }
        }
    };
    Ok(Startup {
        source,
        document,
        store_version,
        directory_generation,
        recovered,
    })
}

/// [`load_startup`], publishing its refusal.
///
/// A read that fails is published as `failed` with the upstream's own words
/// and the process exits with that error.
pub(super) async fn await_startup(
    target: &str,
    local_store: Option<&Arc<RegistryStore>>,
) -> Result<Startup, CmdError> {
    publish(&PublishedState::starting(target));
    load_startup(target, local_store).await.map_err(|detail| {
        publish(&PublishedState::failed(target, &detail));
        CmdError::click(detail).stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}
