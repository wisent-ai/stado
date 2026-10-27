//! How much room the host had when the agent retried a release: the measure
//! a later retry of the same digest has to beat.

use crate::providers::local::host_memory::{gigabytes, read_host_memory};

/// What the host had free when the agent read it: the memory a new
/// allocation can obtain and the free space on the volume holding the state
/// directory, each absent when the host could not read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostRoom {
    pub available_memory_bytes: Option<i64>,
    pub free_disk_bytes: Option<i64>,
}

impl HostRoom {
    /// This host now. A volume whose statistics cannot be read leaves the
    /// disk reading unread, which every comparison here skips rather than
    /// treating as no room.
    pub fn read(state_dir: &str) -> Self {
        // The same reading the disk-full rule takes of a volume.
        let free_disk_bytes =
            match crate::providers::local::space_reclaim::rule::read_volume(std::path::Path::new(state_dir)) {
                Ok(volume) => Some(volume.free_bytes),
                Err(_unreadable_volume) => None,
            };
        Self {
            available_memory_bytes: read_host_memory().available_bytes,
            free_disk_bytes,
        }
    }

    /// Whether this reading has more memory or more disk than `before` had,
    /// each compared only where both were read.
    pub(super) fn exceeds(&self, before: &HostRoom) -> bool {
        let more = |now: Option<i64>, then: Option<i64>| matches!((now, then), (Some(now), Some(then)) if now > then);
        more(self.available_memory_bytes, before.available_memory_bytes)
            || more(self.free_disk_bytes, before.free_disk_bytes)
    }

    /// The larger of each reading, so a retry must beat every earlier one.
    pub(super) fn widest(self, other: HostRoom) -> HostRoom {
        let larger = |left: Option<i64>, right: Option<i64>| match (left, right) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        };
        HostRoom {
            available_memory_bytes: larger(
                self.available_memory_bytes,
                other.available_memory_bytes,
            ),
            free_disk_bytes: larger(self.free_disk_bytes, other.free_disk_bytes),
        }
    }

    pub(super) fn described(&self) -> String {
        let shown = |bytes: Option<i64>| match gigabytes(bytes) {
            Some(gib) => format!("{gib} GiB"),
            None => "unread".to_string(),
        };
        format!(
            "{} of memory and {} of disk free",
            shown(self.available_memory_bytes),
            shown(self.free_disk_bytes)
        )
    }
}
