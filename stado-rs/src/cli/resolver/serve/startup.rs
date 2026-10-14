use std::future::Future;
use std::sync::Arc;

use serde_json::Value;

use crate::monitor::host_silence;
use crate::service_resolution;
use crate::targets::RegistryStore;

use crate::cli::CmdError;

use crate::cli::resolver::directory::document::{kept_last_good_document, last_good_document};
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
        Some(local_store) => {
            let read = within_declared(target, read_local_snapshot(local_store))
                .await
                .and_then(|read| read.map_err(|error| error.to_string()));
            match read {
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
            }
        }
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
        publish(&PublishedState::starting(
            target,
            &format!(
                "reading its first registry snapshot from the authority {} before binding any port",
                source.subject_host(target)
            ),
        ));
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

/// `read`, unless it outlasts the refresh interval the last-known-good
/// registry declares for `target`'s resolver: then why. This host's
/// registry store can be reached only through services whose adapters this
/// very resolver relays — the object API and Skarbiec — and those relay
/// nothing before the first registry read ends, so the read stood for as long
/// as the resolver did, every consumer on the host without a route. Past
/// the interval the resolver serves the last-known-good copy and binds; the
/// refresh loop reads the store again. With no copy, or one that declares
/// no resolver for `target`, there is no measure and the read is waited on;
/// a copy or declaration that cannot be read leaves none either, and says
/// why on stderr, where it used to pass for no copy.
async fn within_declared<T>(target: &str, read: impl Future<Output = T>) -> Result<T, String> {
    let declared = kept_last_good_document().and_then(|copy| match copy {
        Some(document) => service_resolution::declared_resolver_config(&document, target),
        None => Ok(None),
    });
    let seconds = match declared {
        Ok(Some(config)) => config.refresh_seconds,
        Ok(None) => return Ok(read.await),
        Err(cause) => {
            eprintln!(
                "stado resolver: the first registry store read has no refresh interval to stand \
                 within, because the last-known-good registry could not be read ({cause}); it is \
                 waited on until it answers"
            );
            return Ok(read.await);
        }
    };
    let period = std::time::Duration::from_secs(seconds);
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    tokio::pin!(read);
    tokio::select! {
        answer = &mut read => Ok(answer),
        _ = interval.tick() => Err(format!(
            "this host's registry store did not answer within the {seconds}s refresh interval the \
             last-known-good registry declares for {target}; its reads ride adapters this resolver \
             relays only once it serves"
        )),
    }
}

/// [`load_startup`], publishing its refusal.
///
/// A read that fails is published as `failed` with the upstream's own words
/// and the process exits with that error.
pub(super) async fn await_startup(
    target: &str,
    local_store: Option<&Arc<RegistryStore>>,
) -> Result<Startup, CmdError> {
    publish(&PublishedState::starting(
        target,
        "reading this host's registry store before binding any port",
    ));
    load_startup(target, local_store).await.map_err(|detail| {
        publish(&PublishedState::failed(target, &detail));
        CmdError::click(detail).stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}
