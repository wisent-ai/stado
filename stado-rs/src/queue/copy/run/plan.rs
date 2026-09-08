//! The `--dry-run` pass: the same selection and resume arithmetic as a real
//! run, reading both ends and writing neither.

use std::sync::Arc;

use crate::queue::copy::objects::index_by_name;
use crate::queue::copy::report::{CopyOptions, CopyPlan, PrefixPlan};
use crate::queue::{BlobBackend, StorageError};

use super::resume::resume_split;
use super::selected_prefixes;
use super::sentinel::read_sentinel;

/// Plan a copy without writing anything: per-prefix source counts and how
/// much of that is already at the destination.
pub async fn plan(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    options: &CopyOptions,
) -> Result<CopyPlan, StorageError> {
    let prefixes = selected_prefixes(&options.prefixes);
    let sentinel = read_sentinel(destination).await?;
    let (resumed_from, remaining) =
        resume_split(&prefixes, &sentinel.cursor, options.prefixes.is_empty());
    // `remaining` is a suffix of `prefixes`, so everything before it is
    // what the resume cursor would fast-forward past.
    let fast_forwarded = prefixes.len() - remaining.len();
    let mut planned = Vec::new();
    for (index, prefix) in prefixes.iter().enumerate() {
        let blobs = source.list_blobs_with_meta(prefix).await?;
        let present = index_by_name(destination.list_blobs_with_meta(prefix).await?);
        planned.push(PrefixPlan {
            prefix: prefix.clone(),
            source_objects: blobs.len(),
            already_at_destination: blobs
                .iter()
                .filter(|blob| present.contains_key(&blob.name))
                .count(),
            fast_forward: index < fast_forwarded,
        });
    }
    Ok(CopyPlan {
        prefixes: planned,
        resumed_from,
    })
}
