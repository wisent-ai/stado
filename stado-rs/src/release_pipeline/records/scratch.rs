//! How much disk one platform build wrote, measured by the builder before it
//! removed its scratch tree: the evidence that keeps the next build of the
//! same product and platform off a host that cannot hold it.
//!
//! Kept beside the receipt as its own output leaf. [`super::receipt::BuildReceipt`]
//! denies unknown fields, so a reader older than this record must never meet
//! it inside the receipt; a leaf it does not ask for costs it nothing.

use serde::{Deserialize, Serialize};

use crate::release_pipeline::StepStatus;

/// The output leaf the worker writes and the bootstrap uploads beside
/// `receipt.json`.
pub const SCRATCH_LEAF: &str = "scratch.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchReceipt {
    pub schema_version: u32,
    pub run_id: String,
    pub job_id: String,
    pub product: String,
    pub platform: String,
    pub builder: String,
    /// Bytes the worker's scratch tree held when the build step ended: the
    /// extracted source, every declared input, and everything the build wrote.
    pub bytes: u64,
    /// Bytes still free on the filesystem carrying that tree at the same
    /// moment. Near zero on a build that ran out of disk.
    pub free_bytes: u64,
    /// The build step's outcome. A failed build's `bytes` is a floor, not the
    /// need: the build stopped writing when it failed.
    pub build: StepStatus,
    pub measured_at: String,
}

impl ScratchReceipt {
    /// Whether the build stopped for lack of disk, judged from the evidence
    /// alone: it failed with nothing left on the filesystem it was writing.
    pub fn exhausted_disk(&self) -> bool {
        self.build == StepStatus::Failed && self.free_bytes < DISK_EXHAUSTED_BELOW_BYTES
    }
}

/// A failed build that left fewer free bytes than this ran out of disk. One
/// full compilation unit's temporary files are larger, so a build that
/// stopped above it stopped for another reason.
const DISK_EXHAUSTED_BELOW_BYTES: u64 = 256 * 1024 * 1024;

/// Bytes held by every regular file under `root`, following no symlinks.
///
/// Sizes are taken from directory entries' own metadata, so a tree of a
/// hundred thousand build objects costs one stat each and nothing is read.
pub fn tree_bytes(root: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total += metadata.len();
            }
        }
    }
    Ok(total)
}
