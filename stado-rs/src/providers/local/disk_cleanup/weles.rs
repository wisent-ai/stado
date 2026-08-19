//! Weles recordings cleanup: whole-run eviction gated on age, durable
//! upload proof, and run inactivity.
//!
//! Port of the weles half of `stado/providers/local/disk/cleanup.py`
//! (`_weles_upload_proof_ok`, `_weles_run_active`, `_weles_dir_size`,
//! `_scan_weles`). Unlike the HF cleaner this pass is path-based (as in
//! the Python): the safety gate is the ordered series of refusals before
//! `rmtree`, plus the lexical commonpath check — every refusal below is
//! covered by the module test suite.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{euid, fixed_root, free_bytes, CleanupReport, JanitorError, GIB};
use crate::targets::DiskCleanupPolicy;

/// True only when the run carries a valid whole-run upload proof and
/// nothing was written into the run afterwards.
///
/// The proof (recordings/<run>/.uploaded.json, written by the Weles worker
/// after a zero-failure mirror) is invalidated by any newer direct child —
/// a file added after the upload means storage is no longer complete.
/// Python `_weles_upload_proof_ok`.
fn upload_proof_ok(run_dir: &Path) -> bool {
    let proof_path = run_dir.join(".uploaded.json");
    let text = match std::fs::read_to_string(&proof_path) {
        Ok(text) => text,
        Err(_) => return false,
    };
    let proof: Value = match serde_json::from_str(&text) {
        Ok(proof) => proof,
        Err(_) => return false,
    };
    if !proof.is_object() || proof.get("version") != Some(&Value::from(1)) {
        return false;
    }
    match proof.get("file_count") {
        Some(Value::Number(n)) if n.as_i64().is_some_and(|c| c > 0) => {}
        _ => return false,
    }
    let Some(uploaded_at_raw) = proof.get("uploaded_at").and_then(Value::as_str) else {
        return false;
    };
    let uploaded_at = match parse_iso_timestamp(uploaded_at_raw) {
        Some(ts) => ts,
        None => return false,
    };
    let entries = match std::fs::read_dir(run_dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        let Ok(entry) = entry else { return false };
        if entry.file_name() == ".uploaded.json" {
            continue;
        }
        // DirEntry::metadata does not follow symlinks
        // (entry.stat(follow_symlinks=False) parity).
        match entry.metadata() {
            Ok(info) => {
                if info.mtime() as f64 > uploaded_at {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

/// Python `datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp()`.
fn parse_iso_timestamp(raw: &str) -> Option<f64> {
    let replaced = raw.replace('Z', "+00:00");
    let dt = chrono::DateTime::parse_from_rfc3339(&replaced).ok()?;
    let seconds = dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_nanos()) / 1e9;
    Some(seconds)
}

/// Any direct child fresher than cutoff means the run is likely live
/// (dir mtime alone misses in-place file writes). Errors read as
/// inactive: the outer age gate already passed.
/// Python `_weles_run_active`.
fn run_active(path: &Path, cutoff: f64) -> bool {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        match entry.metadata() {
            Ok(info) => {
                if info.mtime() as f64 > cutoff {
                    return true;
                }
            }
            Err(_) => continue,
        }
    }
    false
}

/// Python `_weles_dir_size`: recursive file-size total. os.walk
/// classifies with entry.is_dir() (FOLLOWS symlinks: a symlinked dir is
/// listed but, with followlinks=False, never recursed and never sized);
/// getsize also follows symlinks. Unreadable entries are skipped.
///
/// Shared with [`super::chromium_clones`], which sizes the same shape of
/// thing — one shallow directory of files under a fixed root — and would
/// otherwise be a second walk with its own symlink judgement.
pub(super) fn dir_size(path: &Path) -> i64 {
    let mut total = 0i64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let Ok(target) = std::fs::metadata(&child) else {
                continue;
            };
            if target.is_dir() {
                if !kind.is_symlink() {
                    stack.push(child);
                }
            } else {
                total += target.len() as i64;
            }
        }
    }
    total
}

/// Python `shutil.rmtree(entry.path)`: refuses a top-level symlink, never
/// follows symlinked directories, unlinks everything else. Errors abort
/// the removal and surface to the caller (Python's default onerror).
///
/// Shared with [`super::chromium_clones`] for the reason [`dir_size`] is:
/// two spellings of "delete this tree, refusing symlinks" would be two
/// safety models, and only one of them would be the tested one.
pub(super) fn remove_tree(path: &Path) -> io::Result<()> {
    let info = std::fs::symlink_metadata(path)?;
    if info.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cannot call rmtree on a symbolic link",
        ));
    }
    if !info.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "not a directory",
        ));
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(path)?
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect();
    for child in entries {
        let child_info = std::fs::symlink_metadata(&child)?;
        if child_info.is_dir() && !child_info.file_type().is_symlink() {
            remove_tree(&child)?;
        } else {
            std::fs::remove_file(&child)?;
        }
    }
    std::fs::remove_dir(path)
}

/// Scan the weles recordings root and evict eligible runs.
/// Python `_scan_weles`.
pub fn scan_weles(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get("weles_recordings") else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root = if let Some(configured_root) = &configured.root {
            let expanded = crate::config_file::expand_tilde(configured_root);
            if !expanded.is_dir() {
                report.skip_weles("root_absent", 1);
                return Ok(());
            }
            expanded
        } else {
            let parts = [
                std::ffi::OsString::from("weles"),
                std::ffi::OsString::from("recordings"),
            ];
            match fixed_root(home, &parts, false)? {
                Some(root) => root,
                None => {
                    report.skip_weles("root_absent", 1);
                    return Ok(());
                }
            }
        };
        let mut ordered: Vec<(std::ffi::OsString, PathBuf)> = Vec::new();
        {
            let entries = std::fs::read_dir(&root)?;
            for entry in entries {
                let entry = entry?;
                ordered.push((entry.file_name(), entry.path()));
                if ordered.len() as i64 >= remaining_scan {
                    break;
                }
            }
        }
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let home_device = std::fs::metadata(home)?.dev();
        let mut deleted_bytes = 0i64;
        for (name, path) in ordered {
            report.weles.scanned_items += 1;
            if name == "local" || name.to_string_lossy().starts_with('.') {
                report.skip_weles("reserved_or_hidden", 1);
                continue;
            }
            let info = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_weles("stat_failed", 1);
                    continue;
                }
            };
            if !info.is_dir() || info.file_type().is_symlink() {
                report.skip_weles("not_run_directory", 1);
                continue;
            }
            if info.uid() != euid() || info.dev() != home_device {
                report.skip_weles("unsafe_owner_or_device", 1);
                continue;
            }
            if info.mtime() as f64 > now - configured.min_age_seconds as f64 {
                report.skip_weles("too_young", 1);
                continue;
            }
            if !configured.allow_missing_upload_proof && !upload_proof_ok(&path) {
                // Without durable whole-run upload proof, age alone is never
                // sufficient authorization to delete (default, conservative).
                report.skip_weles("upload_proof_unavailable_v1", 1);
                continue;
            }
            if run_active(&path, now - configured.min_age_seconds as f64) {
                report.skip_weles("active_run", 1);
                continue;
            }
            report.weles.eligible_items += 1;
            let expected = dir_size(&path);
            report.weles.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.weles.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_weles("item_cap", 1);
                continue;
            }
            if deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_weles("byte_cap", 1);
                continue;
            }
            if free_bytes(home)? >= policy.target_free_gb * GIB {
                break;
            }
            // os.path.commonpath([root, entry.path]) != root — lexical
            // escape check; the run dir must stay a direct child.
            if path.parent() != Some(root.as_path()) {
                report.skip_weles("escapes_root", 1);
                continue;
            }
            // Python wraps the free-space probes + rmtree in one
            // try/except (OSError, shutil.Error) per entry.
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree(&path)?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.weles.actual_free_delta_bytes += delta.max(0);
                    report.weles.deleted_items += 1;
                    deleted_bytes += expected;
                }
                Err(exc) => report.add_error("weles_recordings", &exc),
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error("weles_recordings", &exc);
    }
}
