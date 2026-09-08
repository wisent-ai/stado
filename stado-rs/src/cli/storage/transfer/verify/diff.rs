//! One prefix compared: names, metadata, and body bytes.

use crate::cli::storage::*;

/// Result of comparing one common object's body bytes.
struct BodyCheck {
    name: String,
    mismatch: bool,
    error: Option<String>,
}

async fn compare_body(
    source: Arc<dyn BlobBackend>,
    destination: Arc<dyn BlobBackend>,
    name: String,
) -> BodyCheck {
    let (source_body, destination_body) = tokio::join!(
        source.download_bytes(&name),
        destination.download_bytes(&name),
    );
    let outcome = match (source_body, destination_body) {
        (Ok(Some(source_body)), Ok(Some(destination_body))) => {
            return BodyCheck {
                name,
                mismatch: source_body != destination_body,
                error: None,
            };
        }
        (Ok(None), _) => "source object vanished after listing".to_string(),
        (_, Ok(None)) => "destination object vanished after listing".to_string(),
        (Err(error), _) => format!("source body read failed: {error}"),
        (_, Err(error)) => format!("destination body read failed: {error}"),
    };
    BodyCheck {
        name,
        mismatch: false,
        error: Some(outcome),
    }
}

/// Compare one prefix. Read-only: lists metadata and downloads both bodies
/// for every object present on both sides; it never writes or repairs.
pub(in crate::cli::storage) async fn diff_prefix(
    source: &Arc<dyn BlobBackend>,
    destination: &Arc<dyn BlobBackend>,
    prefix: &str,
) -> PrefixDiff {
    let mut diff = PrefixDiff {
        prefix: prefix.to_string(),
        ..PrefixDiff::default()
    };
    let (listed_source, listed_destination) = tokio::join!(
        source.list_blobs_with_meta(prefix),
        destination.list_blobs_with_meta(prefix),
    );
    let source_blobs = match listed_source {
        Ok(blobs) => blobs,
        Err(err) => {
            diff.source_error = Some(err.to_string());
            Vec::new()
        }
    };
    let destination_blobs = match listed_destination {
        Ok(blobs) => blobs,
        Err(err) => {
            diff.destination_error = Some(err.to_string());
            Vec::new()
        }
    };
    if diff.source_error.is_some() || diff.destination_error.is_some() {
        // Counts stay None on purpose: an unreadable side is unknown.
        return diff;
    }

    diff.source_objects = Some(source_blobs.len());
    diff.destination_objects = Some(destination_blobs.len());
    let landed: BTreeMap<String, BTreeMap<String, String>> = destination_blobs
        .into_iter()
        .map(|blob| (blob.name, lowercase_keys(&blob.metadata)))
        .collect();

    let mut source_names: BTreeSet<String> = BTreeSet::new();
    for blob in &source_blobs {
        source_names.insert(blob.name.clone());
        match landed.get(&blob.name) {
            None => diff.missing.push(blob.name.clone()),
            Some(present) => {
                let gaps = metadata_gaps(present, &lowercase_keys(&blob.metadata));
                if !gaps.is_empty() {
                    diff.metadata_gaps.push((blob.name.clone(), gaps));
                }
            }
        }
    }
    diff.extra = landed
        .keys()
        .filter(|name| !source_names.contains(*name))
        .cloned()
        .collect();
    let body_checks: Vec<BodyCheck> = futures::stream::iter(
        source_names
            .iter()
            .filter(|name| landed.contains_key(*name))
            .cloned(),
    )
    .map(|name| compare_body(Arc::clone(source), Arc::clone(destination), name))
    .buffered(copy::DEFAULT_CONCURRENCY)
    .collect()
    .await;
    for check in body_checks {
        if check.mismatch {
            diff.body_mismatches.push(check.name);
        } else if let Some(error) = check.error {
            diff.body_errors.push((check.name, error));
        }
    }

    diff
}

/// Keys `wanted` carries that `landed` does not satisfy.
///
/// This is the rule `queue/copy.rs::metadata_satisfied` enforces after a
/// copy, restated here because that helper is private to the copier and
/// `queue/copy.rs` is unchanged by this command:
///
/// - keys are folded to lowercase, because Azure round-trips metadata
///   through case-insensitive `x-ms-meta-*` headers while GCS preserves the
///   key exactly as written;
/// - empty values are ignored, because
///   `<AzureBlobBackend as BlobBackend>::set_metadata` filters them out
///   before the PUT and they can therefore never land;
/// - extra destination keys are fine, because both backends MERGE on
///   `set_metadata`, so the destination only has to be a superset.
fn metadata_gaps(
    landed: &BTreeMap<String, String>,
    wanted: &BTreeMap<String, String>,
) -> Vec<String> {
    wanted
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .filter(|(key, value)| landed.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect()
}

fn lowercase_keys(metadata: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    metadata
        .iter()
        .map(|(key, value)| (key.to_lowercase(), value.clone()))
        .collect()
}
