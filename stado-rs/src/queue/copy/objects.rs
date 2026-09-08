//! One prefix at a time: the metadata comparison, the per-object copy, the
//! bounded fan-out over a prefix and the mandatory verification pass.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::StreamExt;

use crate::queue::{BlobBackend, BlobInfo, StorageError};

use super::report::{ObjectReport, Outcome, PrefixReport};

/// Index a destination listing by object name, with metadata keys folded to
/// lowercase for comparison (see [`metadata_satisfied`]).
pub(super) fn index_by_name(blobs: Vec<BlobInfo>) -> BTreeMap<String, BTreeMap<String, String>> {
    blobs
        .into_iter()
        .map(|blob| (blob.name, lowercase_keys(&blob.metadata)))
        .collect()
}

fn lowercase_keys(metadata: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    metadata
        .iter()
        .map(|(key, value)| (key.to_lowercase(), value.clone()))
        .collect()
}

/// Whether `landed` already carries everything `wanted` asks for.
///
/// Keys are compared case-insensitively because the two backends disagree
/// on case: Azure round-trips metadata through case-insensitive
/// `x-ms-meta-*` headers, GCS preserves the key exactly as written.
///
/// Empty values are ignored: `<AzureBlobBackend as BlobBackend>::set_metadata`
/// filters empty values out before the PUT, so they can never land and must
/// not be reported as a lost write.
///
/// Extra destination keys are fine — both backends MERGE on `set_metadata`,
/// so the destination is only ever required to be a superset.
fn metadata_satisfied(
    landed: &BTreeMap<String, String>,
    wanted: &BTreeMap<String, String>,
) -> bool {
    wanted
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .all(|(key, value)| landed.get(&key.to_lowercase()) == Some(value))
}

/// Copy one object. Never returns `Err`: a single bad object is recorded in
/// the report so the rest of the run continues and the operator gets the
/// complete list of what needs attention.
async fn copy_object(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    blob: &BlobInfo,
    landed: Option<&BTreeMap<String, String>>,
) -> ObjectReport {
    let name = blob.name.clone();
    let report = |bytes: u64, outcome: Outcome| ObjectReport {
        name: name.clone(),
        bytes,
        outcome,
    };
    let failed = |context: &str, err: StorageError| Outcome::Failed(format!("{context}: {err}"));

    let body = match source.download_bytes(&blob.name).await {
        Ok(Some(body)) => body,
        // Listed a moment ago, gone now: a live queue moved the job.
        Ok(None) => return report(u64::default(), Outcome::Vanished),
        Err(err) => return report(u64::default(), failed("source read failed", err)),
    };
    let size = body.len() as u64;

    // Already there? Compare bytes, not merely lengths. Lifecycle projections
    // and mutable control objects routinely serialize to equal-length bodies;
    // treating size as identity can preserve stale state while reporting a
    // clean copy, after which backup reconciliation is allowed to prune.
    if let Some(landed) = landed {
        let existing = match destination.download_bytes(&blob.name).await {
            Ok(existing) => existing,
            Err(err) => return report(u64::default(), failed("destination read failed", err)),
        };
        if existing.as_ref() == Some(&body) {
            if metadata_satisfied(landed, &blob.metadata) {
                return report(u64::default(), Outcome::Skipped);
            }
            // Body is already right, metadata is not — the exact residue of
            // an earlier run whose Azure metadata PUT was swallowed. Repair
            // the metadata without rewriting the body.
            if let Err(err) = destination.set_metadata(&blob.name, &blob.metadata).await {
                return report(u64::default(), failed("metadata write failed", err));
            }
            return report(u64::default(), Outcome::MetadataRepaired);
        }
    }

    if let Err(err) = destination.upload_bytes(&blob.name, &body).await {
        return report(u64::default(), failed("destination write failed", err));
    }
    if !blob.metadata.is_empty() {
        if let Err(err) = destination.set_metadata(&blob.name, &blob.metadata).await {
            return report(u64::default(), failed("metadata write failed", err));
        }
    }
    report(size, Outcome::Copied)
}

/// Copy every object under one prefix, then verify what landed.
pub(super) async fn copy_prefix(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    prefix: &str,
    concurrency: usize,
) -> PrefixReport {
    let mut report = PrefixReport {
        prefix: prefix.to_string(),
        listing_error: None,
        objects: Vec::new(),
    };
    let blobs = match source.list_blobs_with_meta(prefix).await {
        Ok(blobs) => blobs,
        Err(err) => {
            report.listing_error = Some(format!("source listing failed: {err}"));
            return report;
        }
    };
    if blobs.is_empty() {
        return report;
    }
    let present = match destination.list_blobs_with_meta(prefix).await {
        Ok(existing) => index_by_name(existing),
        Err(err) => {
            report.listing_error = Some(format!("destination listing failed: {err}"));
            return report;
        }
    };

    // Same fan-out idiom as `migrations::backfill_priority_markers`:
    // `buffered` keeps the results aligned with `blobs`, which the
    // verification pass below relies on.
    report.objects = futures::stream::iter(&blobs)
        .map(|blob| copy_object(source, destination, blob, present.get(&blob.name)))
        .buffered(concurrency)
        .collect::<Vec<ObjectReport>>()
        .await;

    verify_metadata(destination, prefix, &blobs, &mut report).await;
    report
}

/// Re-read the destination prefix and confirm the metadata actually landed.
///
/// This pass is mandatory, not paranoia:
/// `<AzureBlobBackend as BlobBackend>::set_metadata` logs and SWALLOWS both
/// a failed request and a non-success response, returning `Ok` either way
/// (Python parity). A successful `set_metadata` therefore carries no
/// information at all, and the scheduler prefilter in `queue::listing`
/// depends on those keys. One listing per prefix re-reads everything the
/// run just wrote.
async fn verify_metadata(
    destination: &Arc<dyn BlobBackend>,
    prefix: &str,
    blobs: &[BlobInfo],
    report: &mut PrefixReport,
) {
    let wrote = |outcome: &Outcome| matches!(outcome, Outcome::Copied | Outcome::MetadataRepaired);
    if !report.objects.iter().any(|object| wrote(&object.outcome)) {
        return;
    }
    let landed = match destination.list_blobs_with_meta(prefix).await {
        Ok(landed) => index_by_name(landed),
        Err(err) => {
            report.listing_error = Some(format!("metadata verification listing failed: {err}"));
            return;
        }
    };
    // `buffered` preserved order, so the two sequences line up.
    for (blob, object) in blobs.iter().zip(report.objects.iter_mut()) {
        if !wrote(&object.outcome) {
            continue;
        }
        match landed.get(&blob.name) {
            None => {
                object.bytes = u64::default();
                object.outcome = Outcome::Failed(
                    "object is absent from the destination listing after the write".into(),
                );
            }
            Some(found) if !metadata_satisfied(found, &blob.metadata) => {
                object.bytes = u64::default();
                object.outcome = Outcome::Failed(format!(
                    "metadata did not land: wanted {:?}, destination has {found:?}",
                    blob.metadata
                ));
            }
            Some(_) => {}
        }
    }
}
