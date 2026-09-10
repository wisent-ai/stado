//! What the last build of a product and platform wrote to disk, and whether
//! one candidate builder has room for that much again.
//!
//! The 0.20.3 darwin build was pinned to charless-mac-mini because it was the
//! first darwin-arm64 host in name order that was above its low watermark. It
//! had fourteen GiB free, compiled for twenty-five minutes, and died writing
//! rustc metadata with no space left. A watermark says when a host is in
//! trouble; it says nothing about whether a build fits. The builder measures
//! its scratch tree before removing it and leaves [`ScratchReceipt`] beside its
//! receipt, and this module turns that record into a placement verdict.

use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::host_gates::RELEASE_SCRATCH_SHORT;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{ScratchReceipt, SCRATCH_LEAF};

/// How many run objects, newest first, are opened looking for evidence
/// before the search gives up. Evidence is nearly always in the first or
/// second, and a product that was never built by a measuring worker must not
/// cost the whole history on every submission.
const EVIDENCE_RUNS_EXAMINED: usize = 40;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// The newest scratch record for `product` on `platform`, or `None` when no
/// measuring worker has built that pair yet. Silence is not a requirement:
/// a build with no evidence is placed the way it always was.
pub(crate) async fn last_scratch(
    store: &JobStorage,
    product: &str,
    platform: &str,
) -> Result<Option<ScratchReceipt>, CmdError> {
    let mut blobs = store
        .list_blobs_with_meta("runs/release-pipeline/")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .into_iter()
        .filter(|blob| blob.name.ends_with("/run.json"))
        .collect::<Vec<_>>();
    blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    for blob in blobs.iter().take(EVIDENCE_RUNS_EXAMINED) {
        let Some(text) = store
            .download_text(&blob.name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
        else {
            continue;
        };
        let Ok(run) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if run["product"].as_str() != Some(product) {
            continue;
        }
        let Some(prefix) = run["platforms"][platform]["output_prefix"].as_str() else {
            continue;
        };
        let Some(bytes) = store
            .read_bytes(&format!(
                "{}{SCRATCH_LEAF}",
                prefix.trim_end_matches('/').to_owned() + "/"
            ))
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

/// The free disk one capacity publication states, in bytes, when it states
/// one at all.
pub(crate) fn published_free_bytes(publication: &Value) -> Option<u64> {
    publication["diag"]["free_disk_gb"]
        .as_f64()
        .filter(|free| *free >= 0.0)
        .map(|free| (free * GIB) as u64)
}

/// The reason a host cannot take this build, or `None` when it can.
///
/// The build must fit above the host's own low watermark: a build that ends
/// exactly at the watermark has already put the host under pressure, and the
/// janitor and every other tenant on it will spend the build's last minutes
/// fighting it for the same bytes. A publication that states no free disk
/// gets no verdict; silence is not a refusal.
pub(crate) fn scratch_verdict(publication: &Value, evidence: &ScratchReceipt) -> Option<String> {
    let free = published_free_bytes(publication)?;
    let low = publication["diag"]["disk_cleanup"]["low_bytes"]
        .as_u64()
        .unwrap_or_default();
    let needed = evidence.bytes.saturating_add(low);
    if free >= needed {
        return None;
    }
    let history = if evidence.exhausted_disk() {
        format!(
            "at least {:.1} GiB before running out of disk on {}",
            evidence.bytes as f64 / GIB,
            evidence.builder
        )
    } else {
        format!(
            "{:.1} GiB on {}",
            evidence.bytes as f64 / GIB,
            evidence.builder
        )
    };
    Some(format!(
        "{RELEASE_SCRATCH_SHORT} ({:.1} GiB free; the last {} build of {} wrote {history}, and \
         needs that above the {:.1} GiB low watermark; reclaim with `stado space reclaim \
         <host> --apply --reason …` or raise the host's target watermark)",
        free as f64 / GIB,
        evidence.platform,
        evidence.product,
        low as f64 / GIB,
    ))
}
