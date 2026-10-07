//! The one build cache the inventory reads: the managed account's fixed
//! Cargo home, its `bin` child, and that directory's complete membership.

use serde::{Deserialize, Serialize};

use super::super::*;
use super::filesystem::metadata_exact;

/// The managed account's fixed Cargo home and bin directory inventory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CargoInventory {
    pub home: FilesystemMetadata,
    pub bin: FilesystemMetadata,
    pub entries: Vec<FilesystemMetadata>,
    /// Every child the script matched; `entries` lists each.
    pub entries_seen: u64,
    /// True only when `entries` names every child.
    pub entries_complete: bool,
    /// `read`, `missing`, `partial_traversal`,
    /// `refused_not_directory`, `refused_unreadable`, or
    /// `refused_parent_not_directory`.
    pub entries_state: String,
    /// True only when both fixed roots and every child were reported without
    /// sanitization, malformed metadata, or an unavailable read.
    pub complete: bool,
}

pub(in crate::deploy::host_inventory) fn settle_cargo_inventory(cargo: &mut CargoInventory) {
    let mut complete = metadata_exact(&cargo.home) && metadata_exact(&cargo.bin);
    cargo
        .entries
        .sort_by(|left, right| left.name.cmp(&right.name));
    cargo.entries_seen = cargo.entries_seen.max(cargo.entries.len() as u64);
    cargo.entries_complete &= matches!(cargo.entries_state.as_str(), "read" | "missing")
        && cargo.entries_seen == cargo.entries.len() as u64;
    for entry in &cargo.entries {
        complete &= metadata_exact(entry)
            && entry.name_state == "read"
            && entry.metadata_state == "read"
            && matches!(entry.symlink_target_state.as_str(), "read" | "not_symlink");
    }
    cargo.entries_complete &= complete;
    cargo.complete &= cargo.entries_complete
        && matches!(cargo.home.metadata_state.as_str(), "read" | "missing")
        && matches!(cargo.bin.metadata_state.as_str(), "read" | "missing");
}
