//! Waiting for the resolver process `pid` to publish `serving`, for a command
//! that just restarted the unit carrying this host's own registry route and
//! has one more registry write to make. Nothing is polled on an interval: the
//! kernel's change notification on the state file's directory wakes the
//! reader each time the resolver publishes, and the resolver's own verdict
//! ends the wait — `serving` with its listeners bound, or `failed` with its
//! reason. A resolver that exits without a verdict is restarted by its unit,
//! and the process that replaces it publishes; that publication ends the
//! wait for the one that exited. The wait stands in this host's register of
//! open waits while it lasts, so a resolver that publishes nothing (one
//! standing on its own registry read) is seen waited on, not a silent hang.
//!
//! The authority-level follows live here too: bringing this host's resolver
//! to the registry the authority serves ([`follow_authority`]), its upstream
//! first, with the waits themselves in [`super::follow`].

use super::follow::follow_version;
use super::published::{published_state, publisher_running, state_path, RESOLVER_SERVING};

const RESOLVER_FAILED: &str = "failed";

/// Block until the resolver process `pid` publishes that it serves, or
/// return its own reason when it publishes that it failed, or say that it
/// exited and which process published since.
pub(crate) fn await_serving(pid: u32) -> Result<(), String> {
    let path = state_path().ok_or_else(|| {
        "the resolver's state file has no location: HOME is unset and STADO_RESOLVER_STATE_FILE is empty"
            .to_string()
    })?;
    let directory = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?
        .to_path_buf();
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    crate::wait::blocking(
        crate::wait::Kind::Process,
        format!("the resolver (pid {pid}) to publish that it serves"),
        path.display(),
        || loop {
            // Armed before the read, so a publication that lands between
            // the read and the wait still wakes it.
            let mut watch =
                crate::queue::local_file::watch_directories(std::slice::from_ref(&directory))
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
            if let Some(state) = published_state() {
                if state.pid == pid && state.state == RESOLVER_SERVING && state.listening {
                    return Ok(());
                }
                if state.pid == pid && state.state == RESOLVER_FAILED {
                    return Err(std::io::Error::other(format!(
                        "the resolver (pid {pid}) failed: {}",
                        match state.reason.as_deref() {
                            Some(reason) => reason,
                            None => "it published no reason",
                        }
                    )));
                }
                if state.pid != pid && publisher_running(&state) {
                    return Err(std::io::Error::other(format!(
                        "the resolver (pid {pid}) exited without publishing that it serves; \
                         pid {} published {} since, at {}",
                        state.pid, state.state, state.updated_at
                    )));
                }
            }
            watch
                .next()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        },
    )
    .map_err(|error| error.to_string())
}

/// [`follow_version`] of the registry the authority serves now. The host this
/// host's resolver reads from answers from its own resolver's held registry
/// (`stado resolver snapshot`), so that resolver follows its store first,
/// through `stado resolver follow` run there over the host channel; then the
/// store version is read from the authority, or from this host's store when
/// this host is the authority, and this host's resolver is brought to it.
/// Returns the directory generation that registry carries. A command that
/// just moved a route waits here before it writes through that route. With
/// no running resolver on this host its reads go to the authority itself, so
/// there is nothing to bring forward: that is a refusal only when
/// `require_resolver` (the operator's own `stado resolver follow`).
pub(crate) async fn follow_authority(require_resolver: bool) -> Result<u64, crate::cli::CmdError> {
    use crate::cli::CmdError;
    if let Some((upstream, output)) = on_upstream_authority("resolver follow").await? {
        if !output.succeeded() {
            return Err(CmdError::click(format!(
                "{upstream}'s resolver, which this host's resolver reads the registry from, did not follow its \
                 registry: {}",
                output.stderr.trim()
            )));
        }
    }
    let (document, version): (serde_json::Value, String) = match crate::cli::resolver::authority_snapshot().await? {
        Some(snapshot) => (serde_json::from_str(&snapshot.content)?, snapshot.version),
        None => crate::cli::registry::fetch_versioned_document().await?,
    };
    let generation = document["service_directory"]["generation"]
        .as_u64()
        .ok_or_else(|| CmdError::declaration("the registry the authority serves names no directory generation".to_string()))?;
    let running = published_state().is_some_and(|state| publisher_running(&state));
    if !running && !require_resolver {
        return Ok(generation);
    }
    // The store version, not the generation: a write that advanced no
    // directory generation (release control) is followed too.
    let wanted = version.clone();
    tokio::task::spawn_blocking(move || follow_version(wanted))
        .await
        .map_err(|error| CmdError::click(format!("the wait for this host's resolver did not end: {error}")))?
        .map_err(|cause| CmdError::click(format!("this host's resolver did not follow registry store version {version}: {cause}")))?;
    Ok(generation)
}

/// Refuse, before anything moves, when the authority this host's resolver
/// reads from runs a Stado without `stado resolver follow`, which
/// [`follow_authority`] runs there after every step; `Ok` when this host's
/// resolver reads its own store or a host the registry no longer names.
pub(crate) async fn ensure_upstream_follows() -> Result<(), crate::cli::CmdError> {
    let Some((upstream, output)) = on_upstream_authority("resolver follow --help").await? else {
        return Ok(());
    };
    if output.succeeded() {
        return Ok(());
    }
    Err(crate::cli::CmdError::refused(format!(
        "{upstream}, whose resolver this host's resolver reads the registry from, runs a Stado without `resolver \
         follow`, so no step could wait for it to hold what the step before wrote; deliver a Stado that carries it \
         to {upstream} first: {}",
        output.stderr.trim()
    )))
}

/// Run `stado <verb>` on the host this host's resolver reads the registry
/// from, when the registry still names that host the authority; `None` when
/// the resolver reads this host's own store, or reads a host the registry no
/// longer names (an authority that moved here: its resolver followed the
/// commit that moved it, and a silent one is not asked).
async fn on_upstream_authority(
    verb: &str,
) -> Result<Option<(String, crate::deploy::CommandOutput)>, crate::cli::CmdError> {
    use crate::cli::CmdError;
    let Some(upstream) = published_state().and_then(|state| state.read_from) else {
        return Ok(None);
    };
    let copy = crate::cli::registry::fetch_document().await?;
    if copy["service_directory"]["authority"]["target"].as_str() != Some(upstream.as_str()) {
        return Ok(None);
    }
    let registry = crate::targets::load_registry_from_value(&copy)
        .map_err(|error| CmdError::declaration(format!("this host's registry does not load: {error}")))?;
    let target = crate::deploy::host_channel::resolve_target(&registry, &upstream)?;
    let script = format!("set -eu\n\"$HOME/.stado/bin/stado\" {verb}\n");
    let output = crate::deploy::host_channel::run_script(target, &script, &crate::deploy::production_runner()).await?;
    Ok(Some((upstream, output)))
}
