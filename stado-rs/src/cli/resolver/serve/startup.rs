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
use crate::cli::resolver::report::published::backoff_delay;
use crate::cli::resolver::report::published::publish;
use crate::cli::resolver::report::published::PublishedState;

/// Why one startup read failed, and whether waiting can help.
enum StartupError {
    /// The same read succeeds later with nothing changed: the object API this
    /// host reads the registry through is not up yet, or the authority is
    /// asleep. Both happen on this fleet every day.
    Transient(String),
    /// Nothing a retry clears.
    Fatal(String),
}

/// Everything `serve` must read before it can bind one port.
pub(super) struct Startup {
    pub(super) source: SnapshotSource,
    pub(super) document: Value,
    pub(super) store_version: String,
    pub(super) directory_generation: u64,
    pub(super) recovered: bool,
}

/// A document that fails `validate_registry` is [`StartupError::Transient`]
/// too, deliberately: the operator republishes the registry and this process
/// picks it up on the next attempt, where exiting would need a restart to
/// notice the fix. The reason is published either way, so a resolver waiting
/// on a malformed document is not a resolver waiting silently.
async fn load_startup(
    target: &str,
    local_store: Option<&Arc<RegistryStore>>,
) -> Result<Startup, StartupError> {
    let (bootstrap, recovered) = match local_store {
        Some(local_store) => match read_local_snapshot(local_store).await {
            Ok((document, _, _)) => (document, false),
            Err(authority_error) => {
                let document = last_good_document().map_err(|cache_error| {
                    StartupError::Transient(format!(
                        "registry authority failed ({authority_error}); recovery registry failed ({cache_error})"
                    ))
                })?;
                eprintln!(
                    "stado resolver recovery: registry authority failed ({authority_error}); bootstrapping routing from the last-known-good registry"
                );
                (document, true)
            }
        },
        None => {
            let document = last_good_document().map_err(StartupError::Transient)?;
            eprintln!(
                "stado resolver recovery: registry backend construction failed; bootstrapping routing from the last-known-good registry"
            );
            (document, true)
        }
    };
    let detected_target = current_target(&bootstrap).map_err(StartupError::Fatal)?;
    if detected_target != target {
        return Err(StartupError::Fatal(format!(
            "resolver target {target:?} does not match this host ({detected_target:?})"
        )));
    }
    let source =
        snapshot_source(local_store.cloned(), &bootstrap, target).map_err(StartupError::Fatal)?;
    let (document, store_version, directory_generation) = if recovered {
        let directory = service_resolution::directory(&bootstrap)
            .map_err(StartupError::Fatal)?
            .ok_or_else(|| {
                StartupError::Fatal("last-known-good registry has no service_directory".to_string())
            })?;
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
            .map_err(StartupError::Transient)?
    };
    Ok(Startup {
        source,
        document,
        store_version,
        directory_generation,
        recovered,
    })
}

/// [`load_startup`] behind a bounded backoff, publishing every refusal.
///
/// The two reads it wraps fail transiently and routinely, and neither is a
/// reason to exit: exiting 69 is what put this service in a restart loop
/// holding a dead ssh control socket, and only repeated `launchctl kickstart`
/// got it out. A registry that says this host is not the declared target
/// still exits immediately, because no amount of waiting fixes it.
pub(super) async fn await_startup(
    target: &str,
    local_store: Option<&Arc<RegistryStore>>,
) -> Result<Startup, CmdError> {
    publish(&PublishedState::starting(target));
    let mut attempt = 0_u32;
    loop {
        match load_startup(target, local_store).await {
            Ok(startup) => return Ok(startup),
            Err(StartupError::Fatal(detail)) => {
                publish(&PublishedState::failed(target, &detail));
                return Err(CmdError::click(detail));
            }
            Err(StartupError::Transient(detail)) => {
                attempt = attempt.saturating_add(1);
                let delay = backoff_delay(attempt);
                eprintln!(
                    "stado resolver upstream read failed, attempt {attempt}, retrying in {}s: {detail}",
                    delay.as_secs()
                );
                publish(&PublishedState::backing_off(
                    target, attempt, &detail, delay,
                ));
                tokio::time::sleep(delay).await;
            }
        }
    }
}
