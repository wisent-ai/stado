//! What the last build of a product and platform wrote to disk, and whether
//! one candidate builder has room for that much again.
//!
//! A build pinned to a host with too little room can compile for half an hour
//! and die writing rustc metadata with no space left — or take the volume to
//! the disk-full threshold, where the janitor deletes everything the fleet put
//! there, the build's own cache included. The builder measures its scratch
//! tree before removing it and leaves [`ScratchReceipt`] beside its receipt,
//! [`super::history`] finds the newest one among the product's own builds, and
//! this module turns that record into a placement verdict.

use serde_json::Value;

use crate::deploy::host_gates::RELEASE_SCRATCH_SHORT;
use crate::release_pipeline::ScratchReceipt;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// The free disk one capacity publication states, in bytes, when it states
/// one at all.
pub(crate) fn published_free_bytes(publication: &Value) -> Option<u64> {
    publication["diag"]["free_disk_gb"]
        .as_f64()
        .filter(|free| *free >= 0.0)
        .map(|free| (free * GIB) as u64)
}

/// The bytes one capacity publication says may still be written before the
/// volume reaches the disk-full threshold, when it states free space and how
/// full the volume is.
fn published_headroom_bytes(publication: &Value, free: u64) -> Option<i64> {
    let used = publication["diag"]["disk_used_percent"]
        .as_f64()
        .filter(|used| (0.0..100.0).contains(used))?;
    let total = (free as f64 / (1.0 - used / 100.0)) as i64;
    Some(crate::providers::local::disk_cleanup::rule::headroom_bytes(
        total,
        i64::try_from(free).ok()?,
    ))
}

/// The reason a host cannot take this build, or `None` when it can.
///
/// The build must fit in the headroom the disk-full rule leaves: a build that
/// takes the volume past the threshold has the janitor delete its own cache
/// under it. A publication that states no free disk gets no verdict; silence
/// is not a refusal.
pub(crate) fn scratch_verdict(publication: &Value, evidence: &ScratchReceipt) -> Option<String> {
    let free = published_free_bytes(publication)?;
    let headroom = published_headroom_bytes(publication, free).unwrap_or(free as i64);
    if headroom >= i64::try_from(evidence.bytes).unwrap_or(i64::MAX) {
        return None;
    }
    // A failed build's bytes are a floor, not the need: it stopped writing
    // when it failed, with the free space it left stated beside it.
    let history = if evidence.build == crate::release_pipeline::StepStatus::Failed {
        format!(
            "at least {:.1} GiB on {} before failing with {:.1} GiB free",
            evidence.bytes as f64 / GIB,
            evidence.builder,
            evidence.free_bytes as f64 / GIB
        )
    } else {
        format!(
            "{:.1} GiB on {}",
            evidence.bytes as f64 / GIB,
            evidence.builder
        )
    };
    Some(format!(
        "{RELEASE_SCRATCH_SHORT} ({:.1} GiB free, {:.1} GiB before the {}% disk-full threshold; \
         the last {} build of {} wrote {history}; `stado space report <host>` shows what holds \
         the volume)",
        free as f64 / GIB,
        headroom as f64 / GIB,
        crate::providers::local::disk_cleanup::rule::DISK_FULL_PERCENT,
        evidence.platform,
        evidence.product,
    ))
}
