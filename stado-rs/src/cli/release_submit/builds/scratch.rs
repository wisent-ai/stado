//! What the last build of a product and platform wrote to disk, and whether
//! one candidate builder has room for that much again.
//!
//! A build pinned to a host with too little room can compile for half an hour
//! and die writing rustc metadata with no space left — or fill the volume
//! until the janitor deletes everything the fleet put there, the build's own
//! cache included. The builder measures its scratch tree before removing it
//! and leaves [`ScratchReceipt`] beside its receipt, [`super::history`] finds
//! the newest one among the product's own builds, and this module turns that
//! record into a placement verdict: the host's free bytes against that
//! scratch plus the janitor's margin the host publishes, the same measure its
//! agent admits work by.

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
/// The build fits when the host's free bytes are above its scratch plus the
/// janitor's margin the host published with its room, whatever percentage of
/// the volume is used. A host whose agent publishes no room is judged as it
/// was before the room was measured: by the headroom under the janitor's
/// rule. A publication that states no volume gets no verdict; silence is not
/// a refusal.
pub(crate) fn scratch_verdict(publication: &Value, evidence: &ScratchReceipt) -> Option<String> {
    use crate::providers::local::space_reclaim::{margin::Room, rule};
    let volume = published_volume(publication)?;
    // A need past i64 does not fit any volume a publication can state.
    let scratch = i64::try_from(evidence.bytes);
    let (fits, against) = match Room::published(&publication["diag"]) {
        Ok(room) => (
            scratch
                .as_ref()
                .is_ok_and(|bytes| volume.free_bytes > bytes.saturating_add(room.margin_bytes)),
            format!(
                "{:.1} GiB needed: this build's scratch and the janitor's margin of {:.1} GiB \
                 ({})",
                gib(evidence.bytes as f64 + room.margin_bytes as f64),
                gib(room.margin_bytes as f64),
                room.margin_source
            ),
        ),
        Err(cause) => {
            let headroom = rule::headroom_bytes(volume.total_bytes, volume.free_bytes);
            (
                scratch.as_ref().is_ok_and(|bytes| headroom >= *bytes),
                format!(
                    "{:.1} GiB before the janitor's {}% rule, since the host publishes no room \
                     ({cause})",
                    gib(headroom as f64),
                    rule::DISK_FULL_PERCENT
                ),
            )
        }
    };
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
        "{RELEASE_SCRATCH_SHORT} ({:.1} GiB free, {against}; the last {} build of {} wrote \
         {history}; `stado space report <host>` shows what holds the volume)",
        gib(volume.free_bytes as f64),
        evidence.platform,
        evidence.product,
    ))
}
