//! What a product's own builds say about placing its next one: the newest
//! scratch record a builder measured, and which builders are compiling it
//! right now. One listing of `runs/build/<product>/` answers both.
//!
//! Builds of one product on one builder share that product's Cargo build
//! directory, and Cargo lets one of them in at a time. Placement used to see
//! none of that: a Stado darwin job was pinned to the host already building
//! Stado and spent 49 minutes on `Blocking waiting for file lock on build
//! directory` while the other darwin builder sat idle.

use std::collections::{BTreeMap, BTreeSet};

use crate::cli::CmdError;
use crate::models::Job;
use crate::queue::storage::JobStorage;
use crate::queue::BlobInfo;
use crate::release_pipeline::{ScratchReceipt, WorkerRequest, SCRATCH_LEAF};

/// The builds of `product` on `platform` whose queue job is still queued or
/// running, read from the queue itself. A request whose job was cancelled,
/// lost or settled is no build in flight however recent it is, and one whose
/// job still runs is in flight however long it has run; a fixed age window
/// (120 minutes, chosen from how long a Stado build took) guessed both.
async fn live_builds(
    store: &JobStorage,
    product: &str,
    platform: &str,
) -> Result<BTreeSet<String>, CmdError> {
    let mut live = BTreeSet::new();
    // The folders, not the states: a queued job lives under `queue/`, and
    // listing `queued/` (the state's name) was refused by every object grant
    // (`object_grant_does_not_cover_key`) and stopped every release submit.
    for state in [
        crate::queue::control::QUEUED_PREFIX,
        crate::queue::control::RUNNING_PREFIX,
    ] {
        let blobs = store
            .list_blobs_with_meta(&format!("{state}/"))
            .await
            .map_err(CmdError::from)?;
        for blob in blobs {
            // A job settled between the listing and this read is not live.
            let Some(text) = store
                .download_text(&blob.name)
                .await
                .map_err(CmdError::from)?
            else {
                continue;
            };
            let job: Job = serde_json::from_str(&text).map_err(|error| {
                CmdError::click(format!(
                    "queue job {} is not a job record: {error}",
                    blob.name
                ))
            })?;
            if crate::providers::local::helpers::build_cache_key(&job) != Some((product, platform))
            {
                continue;
            }
            let build = job
                .output_uri
                .split_once("/runs/build/")
                .and_then(|(_, rest)| {
                    let mut parts = rest.split('/');
                    parts.next();
                    parts.next()
                });
            live.extend(build.map(str::to_string));
        }
    }
    Ok(live)
}

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
        .map_err(CmdError::from)?;
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
    // Newest first until one parses; every record is a candidate, so a run of
    // unreadable ones does not hide an older good one behind a count.
    for blob in records {
        let Some(bytes) = store.read_bytes(&blob.name).await.map_err(CmdError::from)? else {
            continue;
        };
        if let Ok(receipt) = serde_json::from_slice::<ScratchReceipt>(&bytes) {
            return Ok(Some(receipt));
        }
    }
    Ok(None)
}

/// A build is in flight on a platform while its queue job for that platform
/// is still queued or running ([`live_builds`]); its newest request for the
/// platform names the builder it was pinned to.
async fn in_flight(
    store: &JobStorage,
    blobs: &[BlobInfo],
    product: &str,
    platform: &str,
) -> Result<BTreeMap<String, usize>, CmdError> {
    let live = live_builds(store, product, platform).await?;
    let prefix = format!("runs/build/{product}/");
    let first_request = format!("/requests/{platform}.json");
    let retry_request = format!("/requests/{platform}/attempts/");
    // build id -> its newest request for this platform
    let mut builds: BTreeMap<&str, &BlobInfo> = BTreeMap::new();
    for blob in blobs {
        let Some((build, _)) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        if !live.contains(build) {
            continue;
        }
        if !(blob.name.ends_with(&first_request) || blob.name.contains(&retry_request)) {
            continue;
        }
        let newest = builds.entry(build).or_insert(blob);
        if blob.updated > newest.updated {
            *newest = blob;
        }
    }
    let mut counts = BTreeMap::new();
    for request in builds.into_values() {
        let Some(bytes) = store
            .read_bytes(&request.name)
            .await
            .map_err(CmdError::from)?
        else {
            continue;
        };
        if let Ok(request) = serde_json::from_slice::<WorkerRequest>(&bytes) {
            *counts.entry(request.builder).or_insert(0) += 1;
        }
    }
    Ok(counts)
}
