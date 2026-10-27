//! Every read `release status` waits on.
//!
//! The command reads the registry, every target's release state and every
//! recorded release run. Each of those reads is an object API request, and
//! every object API request says on stderr which object it asks for, where
//! and since when before it waits, and how it ended (`crate::wait`), so a
//! status that runs for half an hour names the read it stands on. Stdout,
//! and with it `--json`, is unchanged.

use serde_json::Value;

use crate::cli::release_submit::RunFilter;
use crate::cli::CmdError;

pub(super) async fn registry_document() -> Result<Value, CmdError> {
    crate::cli::registry::fetch_document().await
}

pub(super) async fn registry_targets() -> Result<crate::targets::Registry, CmdError> {
    crate::targets::fetch_registry_remote()
        .await
        .map_err(CmdError::from)
}

/// The release state `target` last wrote for `product`; `None` when it wrote
/// none. A read that fails for any other reason is refused with its cause:
/// shown as no state, a target that reported looked as if it never had.
pub(super) async fn release_state(
    product: &str,
    target: &str,
) -> Result<Option<Vec<u8>>, CmdError> {
    let uri = crate::release_agent::release_status_uri(product, target);
    match crate::cli::storage::fetch_object(&uri).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.failure == Some(crate::primitives::failure::FailureCode::NotFound) => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// [`release_state`] of every `(product, target)` pair, as many at once as
/// the bulk budget allows ([`crate::queue::migrations::bulk_workers`], the
/// one every bulk object read takes), keyed by the pair: each is its own
/// object read, and one after another they stood the store's wait once per
/// target.
pub(super) async fn release_states(
    pairs: &[(String, String)],
) -> Result<std::collections::HashMap<(String, String), Result<Option<Vec<u8>>, CmdError>>, CmdError>
{
    use futures::stream::StreamExt;
    let workers = crate::queue::migrations::bulk_workers().map_err(CmdError::from)?;
    Ok(futures::stream::iter(pairs.iter().cloned())
        .map(|(product, target)| async move {
            let state = release_state(&product, &target).await;
            ((product, target), state)
        })
        .buffered(workers)
        .collect()
        .await)
}

pub(super) async fn recent_runs(
    product: Option<&str>,
    limit: usize,
) -> Result<Vec<Value>, CmdError> {
    crate::cli::release_submit::recent_runs(product, limit).await
}

pub(super) async fn matching_runs(
    filter: RunFilter<'_>,
    limit: usize,
) -> Result<Vec<Value>, CmdError> {
    crate::cli::release_submit::matching_runs(filter, limit).await
}
