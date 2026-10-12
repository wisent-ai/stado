//! The two outcomes of a removal that leave the thing gone.
//!
//! Every host-side delete Stado runs (a file, a run directory, a superseded
//! LaunchAgent plist) reports `removed` when it deleted the thing and `absent`
//! when there was nothing to delete. Both mean the thing is gone; anything
//! else is a refusal or a failure the report names. The two states are named
//! here once, so a caller asks [`gone`] instead of repeating the pair.

pub const REMOVED: &str = "removed";
pub const ABSENT: &str = "absent";

/// The removal left the thing gone: it was deleted, or it was not there.
pub fn gone(status: &str) -> bool {
    status == REMOVED || status == ABSENT
}
