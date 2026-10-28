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
use crate::providers::local::host_memory::constants::GIB;
use crate::release_pipeline::ScratchReceipt;

/// Bytes as GiB for a refusal's sentence.
fn gib(bytes: f64) -> f64 {
    bytes / GIB as f64
}

/// One host's published volume, as its agent measured it: the bytes free to
/// an unprivileged writer and the volume's size.
struct PublishedVolume {
    free_bytes: i64,
    total_bytes: i64,
}

/// The volume one capacity publication states, when it states both figures
/// as byte counts; an agent that could not read its volume publishes them
/// null with `disk_unread` beside them.
fn published_volume(publication: &Value) -> Option<PublishedVolume> {
    let diag = &publication["diag"];
    Some(PublishedVolume {
        free_bytes: diag["disk_free_bytes"].as_i64()?,
        total_bytes: diag["disk_total_bytes"].as_i64()?,
    })
}

/// The free disk one capacity publication states, in bytes, when it states
/// one at all.
pub(crate) fn published_free_bytes(publication: &Value) -> Option<i64> {
    published_volume(publication).map(|volume| volume.free_bytes)
}

/// The reason a host cannot take this build, or `None` when it can.
///
/// The build must fit in the headroom the disk-full rule leaves: a build that
/// takes the volume past the threshold has the janitor delete its own cache
/// under it. A publication that states no volume gets no verdict; silence is
/// not a refusal.
pub(crate) fn scratch_verdict(publication: &Value, evidence: &ScratchReceipt) -> Option<String> {
    let volume = published_volume(publication)?;
    let headroom = crate::providers::local::space_reclaim::rule::headroom_bytes(
        volume.total_bytes,
        volume.free_bytes,
    );
    // A need past i64 does not fit any volume a publication can state.
    let fits = i64::try_from(evidence.bytes).is_ok_and(|need| headroom >= need);
    if fits {
        return None;
    }
    // A failed build's bytes are a floor, not the need: it stopped writing
    // when it failed, with the free space it left stated beside it.
    let history = if evidence.build == crate::release_pipeline::StepStatus::Failed {
        format!(
            "at least {:.1} GiB on {} before failing with {:.1} GiB free",
            gib(evidence.bytes as f64),
            evidence.builder,
            gib(evidence.free_bytes as f64)
        )
    } else {
        format!(
            "{:.1} GiB on {}",
            gib(evidence.bytes as f64),
            evidence.builder
        )
    };
    Some(format!(
        "{RELEASE_SCRATCH_SHORT} ({:.1} GiB free, {:.1} GiB before the {}% disk-full threshold; \
         the last {} build of {} wrote {history}; `stado space report <host>` shows what holds \
         the volume)",
        gib(volume.free_bytes as f64),
        gib(headroom as f64),
        crate::providers::local::space_reclaim::rule::DISK_FULL_PERCENT,
        evidence.platform,
        evidence.product,
    ))
}
