//! The one build cache the inventory reads: the managed account's fixed
//! Cargo home, its `bin` child, and that directory's complete membership.

use serde::{Deserialize, Serialize};

use super::super::*;
use super::filesystem::{clamp, clamp_filesystem_metadata};

/// The managed account's fixed Cargo home and bin directory inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CargoInventory {
    pub home: FilesystemMetadata,
    pub bin: FilesystemMetadata,
    pub entries: Vec<FilesystemMetadata>,
    /// Every child matched, including any beyond [`MAX_CARGO_BIN_ENTRIES`].
    pub entries_seen: u64,
    /// True only when `entries` names every child.
    pub entries_complete: bool,
    /// `read`, `missing`, `partial_traversal`,
    /// `refused_not_directory`, `refused_unreadable`, or
    /// `refused_parent_not_directory`.
    pub entries_state: String,
    /// True only when both fixed roots and every child were reported without
    /// truncation, sanitization, malformed metadata, or an unavailable read.
    pub complete: bool,
}

pub(in crate::deploy::host_inventory) fn clamp_cargo_inventory(cargo: &mut CargoInventory) {
    let home_exact = clamp_filesystem_metadata(&mut cargo.home);
    let bin_exact = clamp_filesystem_metadata(&mut cargo.bin);
    let mut complete = home_exact && bin_exact;
    clamp(&mut cargo.entries_state);
    cargo
        .entries
        .sort_by(|left, right| left.name.cmp(&right.name));
    cargo.entries_seen = cargo.entries_seen.max(cargo.entries.len() as u64);
    cargo.entries.truncate(MAX_CARGO_BIN_ENTRIES);
    cargo.entries_complete &= matches!(cargo.entries_state.as_str(), "read" | "missing")
        && cargo.entries_seen == cargo.entries.len() as u64;
    for entry in &mut cargo.entries {
        complete &= clamp_filesystem_metadata(entry)
            && entry.name_state == "read"
            && entry.metadata_state == "read"
            && matches!(entry.symlink_target_state.as_str(), "read" | "not_symlink");
    }
    cargo.entries_complete &= complete;
    cargo.complete &= cargo.entries_complete
        && matches!(cargo.home.metadata_state.as_str(), "read" | "missing")
        && matches!(cargo.bin.metadata_state.as_str(), "read" | "missing");
}
