//! Reclaim the disaster-recovery replica's proven duplicates, on the janitor's
//! own interval.
//!
//! NO Python original. Measured on `charless-mac-mini` on 2026-08-30, and the
//! measurement is the whole argument for this cleaner. `stado host backup-audit
//! --reclaim-twins --apply` deleted 39,346 replica objects — 48.29 GiB, every
//! one hashed against the primary in that same pass — and took the host from
//! 3.39 GiB free at 83% capacity to 51.82 GiB. Seven minutes later the replica
//! held 15,960 objects / 15.08 GiB again, and free space was back down to
//! 34.5 GiB: about 2 GiB per minute, for as long as the queue keeps draining.
//! A second pass cleared it again. Two operator-typed passes in sixteen minutes
//! is a human holding a valve open.
//!
//! The write side of that loop is closed separately, in
//! [`crate::queue::copy::Endpoint::cannot_replicate`]: a cross-addressed
//! pairing now gets no mirror at all. This cleaner is the other half, and it is
//! not a fallback. It is what makes the disk recover from a replica that
//! already exists — on this host, 24.7 GiB of it — and from any future one,
//! without anybody typing a command, which is the only kind of fix this
//! workspace accepts for something that recurs.
//!
//! **The proof and the deletion are one pass, and that is the entire safety
//! argument.** Every object this cleaner removes is read from both trees and
//! SHA-256'd immediately before its `unlink`. It stores nothing, reads no
//! previous verdict, and trusts no earlier audit — because the classes provably
//! move: the same replica's `absent` class re-resolved into `twin` between two
//! audits an hour apart, once the primary's addresses were repaired, and a
//! deletion driven by the older of those two records would have removed the
//! only copy of objects the primary did not yet hold. The last time a tree on
//! this fleet was assumed to be duplicate it was the sole copy of 9.58 GiB of
//! trained-model artifacts.
//!
//! What it refuses, in order:
//!
//! - a replica path with no counterpart in the primary — the sole-copy case,
//!   which is 9.25 GiB on this host and is never touched;
//! - a counterpart of a different size;
//! - anything that is not a plain file on BOTH sides;
//! - a pair whose two hashes differ;
//! - a pair it did not finish hashing inside its budget.
//!
//! The address mapping is the one [`crate::deploy::host_backup_audit`] uses,
//! and for the same reason: a replica path already under `ecosystem/` is a
//! namespace-qualified store path and maps straight through, while a bare path
//! is what a cross-addressed writer produced and its primary address is that
//! path inside the configured namespace.

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

use super::{euid, free_bytes, CleanupReport, JanitorError, GIB};
use crate::targets::DiskCleanupPolicy;

/// The cleaner's registry name, and the key its counts appear under in the
/// janitor's report. Declared here rather than spelled at each use, because
/// [`crate::targets`]'s allowed-cleaner list, the report and this scan have to
/// name the same cleaner or a policy authorizes a pass that never runs.
pub const CLEANER: &str = "backup_twins";

/// The replica root this cleaner walks, relative to `$HOME`, when the policy
/// names none. Same default as `storage.backup.local.path`.
pub const BACKUP_ROOT: &str = ".stado/local-backup";

/// The primary store root the replica is compared against, relative to `$HOME`.
/// Same default as `storage.local.path`.
pub const PRIMARY_ROOT: &str = ".stado/local-storage";

/// Read size for hashing. One MiB, matching the audit command's remote pass, so
/// the two implementations of this comparison read the same way.
const HASH_CHUNK: usize = 1024 * 1024;

/// SHA-256 of one file, streamed.
fn digest(path: &Path) -> Result<String, JanitorError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![u8::default(); HASH_CHUNK];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// The address in the primary that a replica-relative path belongs to.
fn primary_address(primary: &Path, relative: &Path, namespace: &str) -> PathBuf {
    if relative.starts_with("ecosystem") {
        primary.join(relative)
    } else {
        primary.join("ecosystem").join(namespace).join(relative)
    }
}

/// Collect the replica's files, breadth unbounded but count bounded by the
/// policy's scan budget.
///
/// Ordered by path so a budgeted pass resumes where the previous one stopped
/// being able to look, rather than re-walking the same subtree every interval.
fn candidates(root: &Path, budget: i64, report: &mut CleanupReport) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                report.skip_backup_twins("read_dir_failed", 1);
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => {
                    found.push(path);
                    if found.len() as i64 >= budget {
                        report.caps.scan = true;
                        report.skip_backup_twins("scan_cap", 1);
                        found.sort();
                        return found;
                    }
                }
                _ => report.skip_backup_twins("not_a_plain_file", 1),
            }
        }
    }
    found.sort();
    found
}

/// Delete the replica objects this pass proves identical to the primary.
///
/// `namespace` is the store namespace a bare replica path maps into
/// ([`crate::config::wc_stado_storage_namespace`]); an empty one leaves this
/// cleaner unable to resolve those paths, and it then removes nothing rather
/// than guessing an address.
pub fn scan_backup_twins(
    home: &Path,
    policy: &DiskCleanupPolicy,
    namespace: &str,
    remaining_scan: i64,
    deadline: Instant,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get(CLEANER) else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        if namespace.trim().is_empty() {
            report.skip_backup_twins("namespace_unconfigured", 1);
            return Ok(());
        }
        let backup = match &configured.root {
            Some(root) => crate::config_file::expand_tilde(root),
            None => home.join(BACKUP_ROOT),
        };
        let primary = home.join(PRIMARY_ROOT);
        if !backup.is_dir() {
            report.skip_backup_twins("replica_absent", 1);
            return Ok(());
        }
        if !primary.is_dir() {
            // Without the primary there is nothing to prove a duplicate
            // against, and a replica alone is the only copy.
            report.skip_backup_twins("primary_absent", 1);
            return Ok(());
        }
        let home_device = std::fs::metadata(home)?.dev();
        let mut deleted_bytes = 0i64;
        for path in candidates(&backup, remaining_scan, report) {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                report.skip_backup_twins("scan_deadline", 1);
                break;
            }
            report.backup_twins.scanned_items += 1;
            let Ok(relative) = path.strip_prefix(&backup) else {
                report.skip_backup_twins("escapes_root", 1);
                continue;
            };
            let replica = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_backup_twins("stat_failed", 1);
                    continue;
                }
            };
            // A file this account owns, on the volume the policy's watermarks
            // are measured against. Anything else is either not ours to delete
            // or would not move the number that matters.
            if !replica.is_file()
                || replica.file_type().is_symlink()
                || replica.uid() != euid()
                || replica.dev() != home_device
            {
                report.skip_backup_twins("not_a_plain_owned_file", 1);
                continue;
            }
            let candidate = primary_address(&primary, relative, namespace);
            let Ok(counterpart) = std::fs::symlink_metadata(&candidate) else {
                // The sole-copy case. This is the class that was 9.25 GiB on
                // the host this cleaner was written for, and it is data.
                report.skip_backup_twins("absent_from_primary", 1);
                continue;
            };
            if !counterpart.is_file() || counterpart.file_type().is_symlink() {
                report.skip_backup_twins("primary_not_a_plain_file", 1);
                continue;
            }
            if counterpart.len() != replica.len() {
                report.skip_backup_twins("size_differs", 1);
                continue;
            }
            // Hashing is the expensive half, so it runs only where a size match
            // already makes a twin possible — and it runs HERE, in the same
            // iteration as the unlink below, never from a record.
            let (replica_hash, primary_hash) = match (digest(&path), digest(&candidate)) {
                (Ok(left), Ok(right)) => (left, right),
                _ => {
                    report.skip_backup_twins("unreadable_while_hashing", 1);
                    continue;
                }
            };
            if replica_hash != primary_hash {
                report.skip_backup_twins("content_differs", 1);
                continue;
            }
            report.backup_twins.eligible_items += 1;
            let expected = i64::try_from(replica.len()).unwrap_or(i64::MAX);
            report.backup_twins.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.backup_twins.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_backup_twins("item_cap", 1);
                continue;
            }
            if deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_backup_twins("byte_cap", 1);
                continue;
            }
            if free_bytes(home)? >= policy.target_free_gb * GIB {
                break;
            }
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                std::fs::remove_file(&path)?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.backup_twins.actual_free_delta_bytes += delta.max(0);
                    report.backup_twins.deleted_items += 1;
                    deleted_bytes += expected;
                }
                Err(exc) => report.add_error(CLEANER, &exc),
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error(CLEANER, &exc);
    }
}
