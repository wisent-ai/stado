//! What a product's own builds say about placing its next one: the newest
//! scratch record a builder measured, and which builders are compiling it
//! right now. One listing of `runs/build/<product>/` answers both.
//!
//! Builds of one product on one builder share that product's Cargo build
//! directory, and Cargo lets one of them in at a time. Placement used to see
//! none of that: a Stado darwin job was pinned to the host already building
//! Stado and spent 49 minutes on `Blocking waiting for file lock on build
//! directory` while the other darwin builder sat idle.

use std::collections::BTreeMap;

use chrono::{Duration, Utc};

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::queue::BlobInfo;
use crate::release_pipeline::{ScratchReceipt, WorkerRequest, SCRATCH_LEAF};

/// How many scratch records, newest first, are opened looking for one that
/// parses before the search gives up.
const EVIDENCE_RECORDS_EXAMINED: usize = 40;

/// How long a platform request with no receipt still counts as a build in
/// flight. A Stado build runs 35 to 45 minutes; a request older than this
/// without a receipt belongs to a job that was cancelled or lost, and
/// counting it would push placement away from a host that is idle.
const IN_FLIGHT_WINDOW_MINUTES: i64 = 120;

/// This product's builds, as far as placement on one platform needs them.
#[derive(Default)]
pub(crate) struct ProductBuilds {
    /// The newest scratch record on this platform, when a measuring worker
    /// has left one.
    pub scratch: Option<ScratchReceipt>,
    /// Builds of this product in flight on this platform, by builder target.
    pub in_flight: BTreeMap<String, usize>,
}

pub(crate) async fn read(
    store: &JobStorage,
    product: &str,
    platform: &str,
) -> Result<ProductBuilds, CmdError> {
    let blobs = store
        .list_blobs_with_meta(&format!("runs/build/{product}/"))
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    Ok(ProductBuilds {
        scratch: newest_scratch(store, &blobs, platform).await?,
        in_flight: in_flight(store, &blobs, product, platform).await?,
    })
}

/// A build job's bootstrap leaves the record at
/// `<build>/platforms/<platform>/output/`, or under that platform's
/// `attempts/<id>/output/` for a retry.
async fn newest_scratch(
    store: &JobStorage,
    blobs: &[BlobInfo],
    platform: &str,
) -> Result<Option<ScratchReceipt>, CmdError> {
    let platform_segment = format!("/platforms/{platform}/");
    let leaf = format!("/output/{SCRATCH_LEAF}");
    let mut records: Vec<_> = blobs
        .iter()
        .filter(|blob| blob.name.contains(&platform_segment) && blob.name.ends_with(&leaf))
        .collect();
    records.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    for blob in records.into_iter().take(EVIDENCE_RECORDS_EXAMINED) {
        let Some(bytes) = store
            .read_bytes(&blob.name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
        else {
            continue;
        };
        if let Ok(receipt) = serde_json::from_slice::<ScratchReceipt>(&bytes) {
            return Ok(Some(receipt));
        }
    }
    Ok(None)
}

/// A build is in flight on a platform when its newest request for that
/// platform, written within the window, has no receipt written after it.
/// The request names the builder it was pinned to.
async fn in_flight(
    store: &JobStorage,
    blobs: &[BlobInfo],
    product: &str,
    platform: &str,
) -> Result<BTreeMap<String, usize>, CmdError> {
    let cutoff = Utc::now() - Duration::minutes(IN_FLIGHT_WINDOW_MINUTES);
    let prefix = format!("runs/build/{product}/");
    let first_request = format!("/requests/{platform}.json");
    let retry_request = format!("/requests/{platform}/attempts/");
    let receipt_segment = format!("/platforms/{platform}/");
    // build id -> (newest request, newest receipt time)
    let mut builds: BTreeMap<&str, (Option<&BlobInfo>, Option<chrono::DateTime<Utc>>)> =
        BTreeMap::new();
    for blob in blobs {
        let Some((build, _)) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        let entry = builds.entry(build).or_default();
        if blob.name.ends_with(&first_request) || blob.name.contains(&retry_request) {
            if entry.0.is_none_or(|newest| blob.updated > newest.updated) {
                entry.0 = Some(blob);
            }
        } else if blob.name.contains(&receipt_segment) && blob.name.ends_with("/receipt.json") {
            entry.1 = entry.1.max(blob.updated);
        }
    }
    let mut counts = BTreeMap::new();
    for (request, receipt) in builds.into_values() {
        let Some(request) = request else { continue };
        let Some(requested) = request.updated.filter(|at| *at >= cutoff) else {
            continue;
        };
        if receipt.is_some_and(|at| at >= requested) {
            continue;
        }
        let Some(bytes) = store
            .read_bytes(&request.name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
        else {
            continue;
        };
        if let Ok(request) = serde_json::from_slice::<WorkerRequest>(&bytes) {
            *counts.entry(request.builder).or_insert(0) += 1;
        }
    }
    Ok(counts)
}
