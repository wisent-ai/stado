//! The reading the pass writes down: per-class totals, the exact identities it
//! was asked to compare, and what a reclaim half did.

use std::collections::BTreeMap;

use super::{ABSENT, DIFFERS, SAME_SIZE_UNPROVEN, TWIN};

/// One class's totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassTotals {
    pub objects: u64,
    pub bytes: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectIdentity {
    pub state: String,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectComparison {
    pub path: String,
    pub primary: ObjectIdentity,
    pub backup: ObjectIdentity,
    pub primary_metadata: ObjectIdentity,
    pub backup_metadata: ObjectIdentity,
}

/// The whole reading.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackupAudit {
    pub host: String,
    /// Per-class totals, keyed by [`TWIN`], [`DIFFERS`], [`ABSENT`].
    pub classes: BTreeMap<String, ClassTotals>,
    /// The largest few of each class, for a report that names things rather
    /// than only counting them.
    pub examples: BTreeMap<String, Vec<(u64, String)>>,
    /// Exact primary/backup identities requested by the operator.
    pub objects: Vec<ObjectComparison>,
    /// Backup-visible paths and size metadata from explicitly selected
    /// namespaces. SHA-256 is intentionally absent because this inventory does
    /// not read object bodies.
    pub inventory_objects: Vec<ObjectComparison>,
    /// Immediate directory children under each physical root's `ecosystem/`.
    /// This is metadata-only and proves which API namespaces were considered
    /// without walking or reading their object bodies.
    pub namespaces: BTreeMap<String, Vec<String>>,
    /// True only when both fixed physical roots completed that directory read.
    pub namespace_inventory_complete: bool,
    /// Set when the host could not be classified at all.
    pub unavailable: Option<String>,
    /// True once the remote program printed its end marker, so a truncated
    /// channel is never read as "nothing to reclaim".
    pub complete: bool,
    /// What the pass deleted, and what it would have deleted without
    /// `--apply`. Both are the pass's OWN proof: an object counted here was
    /// hashed on both sides moments before the unlink.
    pub deleted: ClassTotals,
    pub would_delete: ClassTotals,
    /// Deletions the host refused, which leave the replica object in place.
    pub delete_failed: ClassTotals,
    /// Emptied replica directories removed after the deletions.
    pub pruned_directories: i64,
    /// Free 1024-byte blocks on the replica's filesystem, read by this pass on
    /// both sides of its own work.
    pub free_kb_before: Option<i64>,
    pub free_kb_after: Option<i64>,
    /// True once the reclaim half printed its own end marker.
    pub reclaim_complete: bool,
}

impl BackupAudit {
    /// Bytes that are proven present and intact in the primary.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.classes.get(TWIN).map(|t| t.bytes).unwrap_or_default()
    }

    /// Bytes that are data: the primary either lacks them or holds something
    /// else at that address.
    pub fn retained_bytes(&self) -> u64 {
        [DIFFERS, ABSENT, SAME_SIZE_UNPROVEN]
            .iter()
            .filter_map(|class| self.classes.get(*class))
            .map(|totals| totals.bytes)
            .sum()
    }
}
