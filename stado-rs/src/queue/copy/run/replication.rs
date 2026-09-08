//! The coordinator's disaster-recovery pass over the configured endpoints,
//! and the reconciliation that keeps the replica a replica.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::queue::copy::report::{CopyOptions, CopyReport};
use crate::queue::copy::{Endpoint, DEFAULT_CONCURRENCY, SENTINEL_PATH};
use crate::queue::{BlobBackend, StorageError};

use super::copy;

/// Replicate the configured primary store to its disaster-recovery endpoint.
///
/// The coordinator calls this after every dispatch tick. Writes stay
/// single-primary; the backup is never promoted automatically, which avoids
/// split-brain when primary health is uncertain.
/// After a clean copy, stale objects are pruned from canonical backup
/// prefixes so lifecycle moves and deletes remain exact during read failover.
pub async fn replicate_configured_backup() -> Result<Option<CopyReport>, StorageError> {
    let Some(destination_endpoint) = Endpoint::configured_backup() else {
        return Ok(None);
    };
    let source_endpoint = Endpoint::configured_primary();
    // Both refusals live on `Endpoint::cannot_replicate`, because the inline
    // mirror in `JobStorage` has to make exactly the same judgement and a
    // second copy of this rule is how one of the two writers ended up
    // unchecked.
    if let Some(refusal) = source_endpoint.cannot_replicate(&destination_endpoint) {
        return Err(StorageError::Other(refusal));
    }
    let source = source_endpoint.build().await?;
    let destination = destination_endpoint.build().await?;
    let report = copy(
        &source,
        &destination,
        &CopyOptions {
            prefixes: Vec::new(),
            concurrency: DEFAULT_CONCURRENCY,
        },
    )
    .await?;
    if report.is_clean() {
        prune_backup_extras(&source, &destination).await?;
    }
    Ok(Some(report))
}

/// Delete every object in the backup that the source does not have.
///
/// A replica that keeps what the source deleted is not a replica. This used to
/// walk [`CANONICAL_PREFIXES`] only, which left everything outside them
/// accumulating for the lifetime of the host: on charless-mac-mini that was
/// 11.4 GiB of `artifacts/models` and 2.7 GiB of `status/`, and it is why the
/// 47.8 GiB replica had grown larger than the 32.7 GiB primary it mirrors. The
/// operator's decision, recorded here because the reason outlives the diff: the
/// backup mirrors the source in full, and objects the source no longer has are
/// deleted from it on the next replication pass.
///
/// The resume sentinel is the one exception, because the copier writes it to
/// the destination itself ([`SENTINEL_PATH`]) and the source never has it.
/// Deleting it would discard the cursor mid-copy.
///
/// [`CANONICAL_PREFIXES`]: crate::queue::copy::CANONICAL_PREFIXES
async fn prune_backup_extras(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
) -> Result<(), StorageError> {
    let source_names = source
        .list_blobs_with_meta("")
        .await?
        .into_iter()
        .map(|blob| blob.name)
        .collect::<BTreeSet<_>>();
    let destination_names = destination
        .list_blobs_with_meta("")
        .await?
        .into_iter()
        .map(|blob| blob.name)
        .collect::<BTreeSet<_>>();
    for stale in destination_names.difference(&source_names) {
        if stale == SENTINEL_PATH {
            continue;
        }
        if !source.exists(stale).await? {
            destination.delete(stale).await?;
        }
    }
    Ok(())
}
