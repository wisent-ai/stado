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
/// upstream's own words.
async fn load_startup(
    target: &str,
    local_store: Option<&Arc<RegistryStore>>,
) -> Result<Startup, String> {
    let (bootstrap, recovered) = match local_store {
        Some(local_store) => match read_local_snapshot(local_store).await {
            Ok((document, _, _)) => (document, false),
            Err(authority_error) => {
                let document = last_good_document().map_err(|cache_error| {
                    format!(
                        "registry authority failed ({authority_error}); recovery registry failed ({cache_error})"
                    )
                })?;
                eprintln!(
                    "stado resolver recovery: registry authority failed ({authority_error}); bootstrapping routing from the last-known-good registry"
                );
                (document, true)
            }
        },
        None => {
            let document = last_good_document()?;
            eprintln!(
                "stado resolver recovery: registry backend construction failed; bootstrapping routing from the last-known-good registry"
            );
            (document, true)
        }
    };
    let detected_target = current_target(&bootstrap)?;
    if detected_target != target {
        return Err(format!(
            "resolver target {target:?} does not match this host ({detected_target:?})"
        ));
    }
    let source = snapshot_source(local_store.cloned(), &bootstrap, target)?;
    let (document, store_version, directory_generation) = if recovered {
        let directory = service_resolution::directory(&bootstrap)?
            .ok_or_else(|| "last-known-good registry has no service_directory".to_string())?;
        eprintln!(
            "stado resolver recovery: serving the last-known-good generation {} immediately while authority retries continue",
            directory.generation
        );
        (
            bootstrap,
            "last-known-good".to_string(),
            directory.generation,
        )
    } else {
        source
            .fetch(host_silence::READER_RESOLVER)
            .await
            .map_err(|error| error.to_string())?
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
        CmdError::click(detail)
    })
}
