//! A role move that carries the registry authority with it.
//!
//! When a role's source host is also the registry authority, the store the
//! role carries holds the registry the authority writes: once the move fences
//! that store, no ordinary registry commit lands, and the move would be
//! undone at its commit. The route switch and the authority's move are then
//! one registry, read from the authority under the fence (reads stand),
//! committed at the authority through that fence by its own transaction
//! (`registry push --fence-transaction`) — so the authority itself serves a
//! registry naming the destination, and every resolver follows on its next
//! refresh — and installed on the destination, where this runs: in the store
//! just copied there, through its own object API before any resolver routes
//! to it, and in its last-known-good copy.

use serde_json::Value;

use super::super::run_host_script;
use crate::deploy::{shlex_quote, Runner};
use crate::targets::ComputeTarget;

use super::route::switched;
use crate::cli::resolver::{AuthorityHandover, InstalledAuthority};
use crate::cli::CmdError;

/// Refuse unless this Stado runs on `to`, as its last-known-good copy names
/// it: the registry authority is taken on the host that takes it.
pub(in crate::cli::placement) fn ensure_runs_on(to: &str) -> Result<(), CmdError> {
    let copy = crate::cli::resolver::kept_last_good_document()
        .map_err(CmdError::declaration)?
        .ok_or_else(|| {
            CmdError::refused("this host keeps no registry copy to take the authority from")
        })?;
    let here = crate::cli::resolver::current_target(&copy).map_err(CmdError::declaration)?;
    if here != to {
        return Err(CmdError::refused(format!(
            "the registry authority is taken on the host that takes it: run this on {to}, not {here}"
        )));
    }
    Ok(())
}

/// Refuse a role move that cannot commit as asked: a role whose source holds
/// the registry authority moves only carrying it (`--with-authority`, run on
/// the destination), and carrying it from a source that does not hold it
/// has nothing to carry.
pub(super) fn ensure_carriable(
    document: &Value,
    service: &str,
    source: &str,
    to_host: &str,
    with_authority: bool,
) -> Result<(), CmdError> {
    let authority = document["service_directory"]["authority"]["target"].as_str();
    let source_is_authority = authority == Some(source);
    if source_is_authority && !with_authority {
        return Err(CmdError::refused(format!(
            "{source} holds the registry authority, and {service}'s store holds the registry it writes, so no \
             registry commit can land once the move fences that store; move it with --with-authority, run on {to_host}"
        )));
    }
    if with_authority && !source_is_authority {
        return Err(CmdError::refused(format!(
            "--with-authority carries the registry authority from {service}'s source {source}, but the directory names {} \
             the authority",
            crate::cli::reporting::optional_text(authority)
        )));
    }
    if with_authority {
        ensure_runs_on(to_host)?;
    }
    Ok(())
}

/// Refuse, before anything is fenced, a source whose installed Stado cannot
/// take the registry commit through its own fence: its `registry push`, run
/// as the directory names the authority's command, offers no
/// `--fence-transaction`, so the commit would be refused and the move undone.
pub(super) async fn ensure_source_takes_fenced_commit(
    document: &Value,
    source: &ComputeTarget,
    runner: &Runner,
) -> Result<(), CmdError> {
    let command = document["service_directory"]["authority"]["command"]
        .as_str()
        .ok_or_else(|| {
            CmdError::declaration("service_directory.authority names no command".to_string())
        })?;
    let script = format!("set -eu\n{} registry push --help\n", shlex_quote(command));
    let output = run_host_script(source, &script, runner, "registry push --help").await?;
    if !output.stdout.contains("--fence-transaction") {
        return Err(CmdError::refused(format!(
            "{}: its Stado ({command}) offers no registry push --fence-transaction, so the registry commit through its \
             fence would be refused; deliver a Stado that carries it to {} before moving the role",
            source.name, source.name
        )));
    }
    Ok(())
}

/// Refuse, before anything is fenced, a destination whose own object API
/// does not answer a registry read at the catalog's address (this runs on
/// the destination): the registry is written there once the authority's
/// commit names it, and a store write that fails then cannot be undone.
pub(super) async fn ensure_destination_store_answers(
    served: &str,
    to_host: &str,
) -> Result<(), CmdError> {
    let unanswered = |error: String| {
        CmdError::refused(format!(
            "{to_host}'s own object API at {served} did not answer a registry read, so the registry could not be \
             written there after the move: {error}"
        ))
    };
    let store = crate::targets::RegistryStore::at_object_api(served)
        .map_err(|error| unanswered(error.to_string()))?;
    store
        .read_versioned()
        .await
        .map_err(|error| unanswered(error.to_string()))?;
    Ok(())
}

/// The route switch and the authority's move as one registry, committed at
/// the authority and installed here. `Err` when nothing was committed (the
/// move is undone); otherwise what the install did, whose failure names the
/// store it did not reach.
pub(super) async fn carried(
    service: &str,
    source: &str,
    to_host: &str,
    served: &str,
    transaction: &str,
) -> Result<InstalledAuthority, CmdError> {
    let unread = || {
        CmdError::refused(format!(
            "this host's registry copy names no other authority, so the registry cannot be committed at {source} \
             before its store moves"
        ))
    };
    let snapshot = crate::cli::resolver::authority_snapshot()
        .await?
        .ok_or_else(unread)?;
    let current: Value = serde_json::from_str(&snapshot.content)?;
    let next = switched(&current, service, source, to_host, served)?;
    let command = crate::cli::resolver::own_command()?;
    let promoted = crate::cli::resolver::promote_authority(&next, to_host, &command)?;
    let content = serde_json::to_string_pretty(&promoted)?;
    crate::cli::resolver::authority_compare_and_swap(
        &snapshot.version,
        &content,
        Some(transaction),
    )
    .await?
    .ok_or_else(unread)?;
    let handover = AuthorityHandover {
        to_host,
        old: source,
        unanswered: None,
        store_at: Some(served),
    };
    let mut installed = crate::cli::resolver::install_authority(&content, handover).await?;
    // Every write after this goes through the route just moved here, which
    // this host resolves once its resolver holds the new generation: the
    // resolver it reads from follows first, then this one. A failure here
    // stands beside the committed move, never undoes it.
    if installed.failure.is_none() {
        if let Err(cause) = crate::cli::resolver::follow_authority(false).await {
            installed.failure = Some(cause.also(format!(
                "{to_host}'s resolver did not follow the registry that moved {service} here, so this host still \
                 resolves the old route; stado resolver follow, run here, brings it forward"
            )));
        }
    }
    Ok(installed)
}
