//! A registry write on a host whose registry authority is another host, sent
//! to that authority over the native SSH channel its reads already use.
//!
//! [`crate::targets::RegistryStore`] reads the registry through the
//! authority's snapshot, but its compare-and-swap went to the configured
//! store, which on such a host is the authority's object API reached through
//! this host's own resolver adapter. A resolver that refused to start
//! therefore took every registry write with it, including the write that
//! removes the declaration it refused on: the one way out the refusal names
//! could not be taken. The swap now runs as `stado registry push -` on the
//! authority, conditional on the generation the caller read, so it reaches
//! the same store without passing this host's resolver.

use serde_json::Value;

use crate::cli::registry::REGISTRY_CONFLICT_EXIT;
use crate::cli::resolver::authority::execute::{execute_with_input, Output};
use crate::cli::resolver::directory::document::last_good_document;
use crate::cli::resolver::directory::source::{current_target, snapshot_source, SnapshotSource};
use crate::queue::StorageError;

/// Replace the registry on the authority iff it is still at `expected`, and
/// return the generation it now carries. `Ok(None)` when this host keeps no
/// copy naming an authority, or is the authority: the caller swaps in its
/// store. A lost condition is [`StorageError::StorageConflict`], as the
/// store's own swap reports it.
pub(crate) async fn authority_compare_and_swap(
    expected: &str,
    content: &str,
) -> Result<Option<String>, StorageError> {
    let Ok(copy) = last_good_document() else {
        return Ok(None);
    };
    let Some(directory) =
        crate::service_resolution::directory(&copy).map_err(StorageError::Other)?
    else {
        return Ok(None);
    };
    let local_target = current_target(&copy).map_err(StorageError::Other)?;
    if directory.authority.target == local_target {
        return Ok(None);
    }
    let SnapshotSource::Authority {
        target,
        ssh,
        command,
    } = snapshot_source(None, &copy, &local_target).map_err(StorageError::Other)?
    else {
        return Ok(None);
    };
    // The guards a whole-document replace earns already ran here against the
    // document at `expected`; the authority's swap is conditional on that
    // same generation, so its own guards are told the decision was made.
    let remote = format!(
        "{} registry push - --if-generation {} --force --allow-empty-fleet --json",
        crate::deploy::shlex_quote(&command),
        crate::deploy::shlex_quote(expected)
    );
    let mut failures = Vec::new();
    for path in &ssh {
        match execute_with_input(&path.destination, &remote, content.as_bytes()).await {
            Ok(output) => return swapped(&target, expected, &output).map(Some),
            Err(error) => failures.push(format!("{}: {error:#}", path.name)),
        }
    }
    Err(StorageError::Other(format!(
        "registry write through authority {target}: no SSH connection path answered ({})",
        failures.join("; ")
    )))
}

/// The authority's answer: the generation its receipt names, the conflict its
/// exit status declares, or its own refusal.
fn swapped(target: &str, expected: &str, output: &Output) -> Result<String, StorageError> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let receipt = serde_json::from_slice::<Value>(&output.stdout);
    let generation = receipt
        .as_ref()
        .ok()
        .and_then(|receipt| receipt.get("generation"))
        .and_then(Value::as_str);
    if let Some(generation) = generation {
        return Ok(generation.to_string());
    }
    let status = output
        .exit_status
        .and_then(|status| i32::try_from(status).ok());
    if status == Some(REGISTRY_CONFLICT_EXIT) {
        return Err(StorageError::StorageConflict(format!(
            "registry authority {target} refused the write conditional on generation {expected}: {}",
            stderr.trim()
        )));
    }
    let status = output
        .exit_status
        .map(|status| status.to_string())
        .ok_or_else(|| {
            StorageError::Other(format!(
                "registry authority {target} closed the write channel without an exit status: {}",
                stderr.trim()
            ))
        })?;
    // An exit with no generation is said with what the receipt was, so a
    // write the authority took but answered unreadably is not read as a
    // plain refusal.
    let unread = match &receipt {
        Ok(receipt) => format!("its receipt names no generation: {receipt}"),
        Err(error) => format!("its receipt does not parse: {error}"),
    };
    Err(StorageError::Other(format!(
        "registry authority {target} exited {status} with no generation ({unread}): {}",
        stderr.trim()
    )))
}
