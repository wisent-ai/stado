//! Driving a whole run: prefix selection and the resumable copy loop.
//!
//! The pieces the loop leans on live beside it: `sentinel` reads and writes
//! the destination resume marker, `resume` turns that cursor into the
//! prefixes still to walk, `plan` answers the same question without writing
//! anything, and `replication` is the coordinator's disaster-recovery pass
//! over the configured endpoints.

mod plan;
mod replication;
mod resume;
mod sentinel;

use std::sync::Arc;

use crate::queue::copy::objects::copy_prefix;
use crate::queue::copy::report::{CopyOptions, CopyReport};
use crate::queue::copy::CANONICAL_PREFIXES;
use crate::queue::{BlobBackend, StorageError};

use resume::resume_split;
use sentinel::{read_sentinel, write_sentinel};

pub use plan::plan;
pub use replication::replicate_configured_backup;

/// The prefixes a run will walk: the explicit selection, or the canonical
/// set when none was given.
fn selected_prefixes(requested: &[String]) -> Vec<String> {
    if requested.is_empty() {
        return CANONICAL_PREFIXES
            .iter()
            .map(|prefix| (*prefix).to_string())
            .collect();
    }
    requested.to_vec()
}

/// Copy every selected prefix from `source` to `destination`.
///
/// Never deletes. The resume cursor advances only while prefixes finish
/// clean and in order — once one fails, later prefixes are still copied
/// (best effort) but the cursor stops, so the next run retries from the
/// last known-good point.
pub async fn copy(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    options: &CopyOptions,
) -> Result<CopyReport, StorageError> {
    let prefixes = selected_prefixes(&options.prefixes);
    let mut sentinel = read_sentinel(destination).await?;
    let (resumed_from, remaining) =
        resume_split(&prefixes, &sentinel.cursor, options.prefixes.is_empty());

    let mut report = CopyReport {
        prefixes: Vec::new(),
        resumed_from,
    };
    let mut cursor_open = true;
    for prefix in remaining {
        let prefix_report = copy_prefix(source, destination, prefix, options.concurrency).await;
        sentinel.copied = sentinel
            .copied
            .saturating_add(prefix_report.copied() as u64);
        sentinel.repaired = sentinel
            .repaired
            .saturating_add(prefix_report.repaired() as u64);
        sentinel.skipped = sentinel
            .skipped
            .saturating_add(prefix_report.skipped() as u64);
        sentinel.vanished = sentinel
            .vanished
            .saturating_add(prefix_report.vanished() as u64);
        sentinel.failed = sentinel
            .failed
            .saturating_add(prefix_report.failed() as u64);
        sentinel.bytes = sentinel.bytes.saturating_add(prefix_report.bytes());
        cursor_open = cursor_open && prefix_report.is_clean();
        if cursor_open {
            sentinel.cursor = prefix.clone();
        }
        // Persist after every prefix so an interrupted run resumes here.
        write_sentinel(destination, &sentinel).await?;
        report.prefixes.push(prefix_report);
    }
    if cursor_open {
        // Clean run: drop the cursor so the next invocation is a full
        // re-sync rather than a no-op.
        sentinel.cursor = String::new();
        write_sentinel(destination, &sentinel).await?;
    }
    Ok(report)
}
