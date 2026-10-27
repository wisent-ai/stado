//! The registry's primary-reads store on a host whose primary and mirror
//! are both directories, opened by the first read and kept for the life of
//! the process.
//!
//! Opened per read, every registry read of a long-running process (the
//! object API's control routes, every fetch of the registry) built a whole
//! storage facade, checked its layout and mirror pairing and logged the
//! mirror refusal each time. A store with a network client is not kept: its
//! pooled connections live on the runtime that opened it, and each
//! `stado serve` component and each test has its own. A failed opening is
//! not kept either, so the next read tries again with its own cause.

use crate::capabilities::StorageAdapter;
use crate::queue::{JobStorage, StorageError};

static KEPT: tokio::sync::OnceCell<JobStorage> = tokio::sync::OnceCell::const_new();

/// Whether this process's store (`primary`) and its configured mirror are
/// directories only, so a facade opened on one runtime holds nothing
/// another runtime cannot use.
fn files_only(primary: Option<StorageAdapter>) -> bool {
    let mirror = crate::config::wc_backup_storage_backend();
    primary == Some(StorageAdapter::Local)
        && (mirror.is_empty()
            || crate::capabilities::storage_adapter(mirror) == Some(StorageAdapter::Local))
}

/// The primary-reads store, kept for the process when [`files_only`] holds
/// and opened afresh otherwise.
pub(super) async fn primary_reads(
    primary: Option<StorageAdapter>,
) -> Result<JobStorage, StorageError> {
    if !files_only(primary) {
        return JobStorage::for_primary_reads().await;
    }
    KEPT.get_or_try_init(JobStorage::for_primary_reads)
        .await
        .cloned()
}
