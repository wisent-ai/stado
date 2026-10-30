//! The janitor's record of what it removed: one JSON line per removal, kept
//! beside its state (`reclaimed.jsonl` in `~/.cache/wisent-compute`).
//!
//! The state file keeps the last pass's counts; a count says how many trees
//! went, not which. An operator who finds tens of GiB gone from a disk, or a
//! user who wants to know what the janitor took while she was away, reads
//! this file (`stado space candidates` prints its newest rows). Each
//! build-cache tree a pass removes is one line naming its path and bytes;
//! every other cleaner's deletions in one pass are one line with its item
//! count and bytes, since those cleaners remove single files by the
//! thousand.
//!
//! Only the janitor appends; nothing rewrites a line. A line that is not one
//! JSON object is refused when the file is read, naming the line.

use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::Class;
use crate::providers::local::space_reclaim::janitor::state::report::build::utc_now;
use crate::providers::local::space_reclaim::{
    agent_logs, backup_twins, chromium_clones, compiler_cache, delivered_releases, euid,
    job_outputs, local_snapshots, object_evidence, queue_workdirs, release_store, weles,
    CleanerReport, CleanupReport, JanitorError,
};

/// The record's name in the janitor's state directory.
pub const LEDGER_NAME: &str = "reclaimed.jsonl";

/// The record in `state_dir`.
pub fn ledger_path(state_dir: &Path) -> PathBuf {
    state_dir.join(LEDGER_NAME)
}

/// Append one line. The file is the account's own regular file, opened
/// without following a link and created owner-only.
fn append(state_dir: &Path, row: &Value) -> Result<(), JanitorError> {
    let path = ledger_path(state_dir);
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .mode(crate::primitives::file_mode::owner_read_write())
        .open(&path)
        .map_err(|error| {
            JanitorError::os(&format!(
                "the removal record {} cannot be opened for appending: {error}",
                path.display()
            ))
        })?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != euid() {
        return Err(JanitorError::os(&format!(
            "the removal record {} is not this account's regular file, so nothing is appended \
             to it; move it aside and the next removal starts a new one",
            path.display()
        )));
    }
    let mut line = serde_json::to_vec(row)?;
    line.push(b'\n');
    file.write_all(&line).map_err(|error| {
        JanitorError::os(&format!(
            "the removal record {} cannot be written: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

/// One tree an enforcing pass removed whole: a tagged build tree
/// (`build_caches`) or a Cargo build directory (`cargo_build_dirs`).
pub(in crate::providers::local::space_reclaim) fn record_tree(
    state_dir: &Path,
    cleaner: &str,
    writer: &str,
    path: &Path,
    bytes: i64,
    freed_bytes: i64,
) -> Result<(), JanitorError> {
    append(
        state_dir,
        &json!({
            "removed_at": utc_now(),
            "writer": writer,
            "cleaner": cleaner,
            "class": Class::AgentOutput.word(),
            "path": path,
            "bytes": bytes,
            "freed_bytes": freed_bytes,
        }),
    )
}

/// Every cleaner that removes items by the file rather than trees
/// [`record_tree`] records one by one.
fn per_pass_cleaners(report: &CleanupReport) -> [(&'static str, &CleanerReport); 12] {
    [
        ("huggingface_cache", &report.hf),
        (weles::CLEANER, &report.weles),
        (chromium_clones::CLEANER, &report.clones),
        (queue_workdirs::CLEANER, &report.workdirs),
        (job_outputs::CLEANER, &report.job_outputs),
        (backup_twins::CLEANER, &report.backup_twins),
        (release_store::CLEANER, &report.release_store),
        (local_snapshots::CLEANER, &report.local_snapshots),
        (object_evidence::CLEANER, &report.object_evidence),
        (agent_logs::CLEANER, &report.agent_logs),
        (delivered_releases::CLEANER, &report.delivered_releases),
        (compiler_cache::CLEANER, &report.compiler_cache),
    ]
}

/// One line for each cleaner that removed anything in an enforcing pass.
pub(in crate::providers::local::space_reclaim) fn record_pass(
    state_dir: &Path,
    report: &CleanupReport,
) -> Result<(), JanitorError> {
    for (cleaner, record) in per_pass_cleaners(report) {
        if record.deleted_items <= 0 {
            continue;
        }
        append(
            state_dir,
            &json!({
                "removed_at": utc_now(),
                "writer": report.writer,
                "cleaner": cleaner,
                "items": record.deleted_items,
                "bytes": record.expected_bytes,
                "freed_bytes": record.actual_free_delta_bytes,
            }),
        )?;
    }
    Ok(())
}

/// The newest `limit` lines, newest first. An absent record is no removal
/// yet; a line that is not one JSON object is refused naming the line.
pub fn newest(state_dir: &Path, limit: usize) -> Result<Vec<Value>, String> {
    let path = ledger_path(state_dir);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "the janitor's removal record {} cannot be opened: {error}; it must be this \
                 account's regular file (a link is refused): move it aside and the next removal \
                 starts a new one",
                path.display()
            ))
        }
    };
    let mut rows = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let number = index + 1;
        let line = line.map_err(|error| {
            format!(
                "line {number} of the janitor's removal record {} cannot be read: {error}; \
                 move the file aside and the next removal starts a new one",
                path.display()
            )
        })?;
        match serde_json::from_str::<Value>(&line) {
            Ok(row) if row.is_object() => rows.push(row),
            Ok(_) | Err(_) => {
                return Err(format!(
                    "line {number} of the janitor's removal record {} is not one JSON object; \
                     only the janitor appends to it, so the file was edited or damaged: move it \
                     aside and the next removal starts a new one",
                    path.display()
                ))
            }
        }
    }
    rows.reverse();
    rows.truncate(limit);
    Ok(rows)
}
