//! How much disk one platform build wrote, measured by the builder before it
//! removed its scratch tree: the evidence that keeps the next build of the
//! same product and platform off a host that cannot hold it.
//!
//! Kept beside the receipt as its own output leaf. [`super::receipt::BuildReceipt`]
//! denies unknown fields, so a reader older than this record must never meet
//! it inside the receipt; a leaf it does not ask for costs it nothing.
//!
//! The builder also keeps its newest receipt per product and platform on its
//! own host, under [`HOST_RECORD_DIR`] beside the janitor's state
//! ([`record_on_host`]): the agent there reads the largest recent one
//! ([`largest_on_host`]) as part of the room work needs before it claims.

use std::path::Path;

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
    /// moment.
    pub free_bytes: u64,
    /// The build step's outcome. A failed build's `bytes` is a floor, not the
    /// need: the build stopped writing when it failed.
    pub build: StepStatus,
    pub measured_at: String,
}

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

/// The directory beside the janitor's state that holds this host's newest
/// scratch receipt per product and platform, one `<product>@<platform>.json`
/// each, so it holds at most one file per pair this host builds.
pub const HOST_RECORD_DIR: &str = "release-scratch";

/// What a record file's name ends with; a staging file a writer has not yet
/// renamed ends otherwise.
const RECORD_SUFFIX: &str = ".json";

/// What a staging file's name ends with until it is renamed into place.
const STAGING_SUFFIX: &str = ".tmp";

/// Whether a product or platform name can be a record file's name part.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// When `receipt` was measured, or why its time is not one.
fn measured(receipt: &ScratchReceipt, file: &str) -> Result<chrono::DateTime<chrono::Utc>, String> {
    chrono::DateTime::parse_from_rfc3339(&receipt.measured_at)
        .map(|at| at.with_timezone(&chrono::Utc))
        .map_err(|error| {
            format!(
                "{file} in this host's {HOST_RECORD_DIR} record states measured_at {:?}, not a \
                 time: {error}; the next release build of that product and platform here \
                 replaces it",
                receipt.measured_at
            )
        })
}

/// Keep `receipt` as this host's newest scratch of its product and platform
/// in `state_dir`, and remove the receipts measured before `since` and the
/// staging files writers left before it. The file is written whole under a
/// staging name and renamed into place, so a reader never meets half of it.
pub fn record_on_host(
    state_dir: &Path,
    receipt: &ScratchReceipt,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    if !plain(&receipt.product) || !plain(&receipt.platform) {
        return Err(format!(
            "product {:?} on platform {:?} cannot name a {HOST_RECORD_DIR} record file: each \
             must be letters, digits, '-', '_' or '.' and not start with '.'",
            receipt.product, receipt.platform
        ));
    }
    let directory = state_dir.join(HOST_RECORD_DIR);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("{} cannot be made: {error}", directory.display()))?;
    let name = format!("{}@{}{RECORD_SUFFIX}", receipt.product, receipt.platform);
    let staging = directory.join(format!("{name}.{}{STAGING_SUFFIX}", std::process::id()));
    let body = serde_json::to_vec(receipt).map_err(|error| format!("{name}: {error}"))?;
    let written = std::fs::write(&staging, body)
        .and_then(|()| std::fs::rename(&staging, directory.join(&name)));
    if let Err(error) = written {
        let left = match std::fs::remove_file(&staging) {
            Ok(()) => String::new(),
            Err(removal) => format!("; its staging file {} stays: {removal}", staging.display()),
        };
        return Err(format!(
            "{} cannot be written: {error}{left}",
            directory.join(&name).display()
        ));
    }
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| format!("{} cannot be listed: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let path = entry.path();
        let file = entry.file_name().to_string_lossy().into_owned();
        let old = if file.ends_with(STAGING_SUFFIX) {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(|error| format!("{}: {error}", path.display()))?;
            chrono::DateTime::<chrono::Utc>::from(modified) < since
        } else if file.ends_with(RECORD_SUFFIX) {
            let bytes =
                std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            match serde_json::from_slice::<ScratchReceipt>(&bytes) {
                Ok(kept) => measured(&kept, &file)? < since,
                // Not a receipt: nothing reads it as one, and the next build of
                // its pair writes it again.
                Err(_) => true,
            }
        } else {
            false
        };
        if old {
            std::fs::remove_file(&path)
                .map_err(|error| format!("{} cannot be removed: {error}", path.display()))?;
        }
    }
    Ok(())
}

/// The largest scratch receipt in `state_dir`'s [`HOST_RECORD_DIR`] measured
/// at or after `since`; `None` with none. The cause names the record without
/// its local path, since it is published with the host's capacity.
pub fn largest_on_host(
    state_dir: &Path,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<Option<ScratchReceipt>, String> {
    let directory = state_dir.join(HOST_RECORD_DIR);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "this host's {HOST_RECORD_DIR} record beside the janitor's state cannot be \
                 listed: {error}"
            ))
        }
    };
    let mut largest: Option<ScratchReceipt> = None;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!("this host's {HOST_RECORD_DIR} record cannot be listed: {error}")
        })?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if !file.ends_with(RECORD_SUFFIX) {
            continue;
        }
        let bytes = std::fs::read(entry.path()).map_err(|error| {
            format!("{file} in this host's {HOST_RECORD_DIR} record cannot be read: {error}")
        })?;
        let receipt = serde_json::from_slice::<ScratchReceipt>(&bytes).map_err(|error| {
            format!(
                "{file} in this host's {HOST_RECORD_DIR} record is not a scratch receipt: \
                 {error}; the next release build of that product and platform here replaces it"
            )
        })?;
        if measured(&receipt, &file)? < since {
            continue;
        }
        if largest
            .as_ref()
            .is_none_or(|kept| receipt.bytes > kept.bytes)
        {
            largest = Some(receipt);
        }
    }
    Ok(largest)
}
