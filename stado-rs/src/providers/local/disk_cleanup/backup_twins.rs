//! Remove replica files only after comparing them with the primary in the
//! same pass. Bounded passes resume a durable frontier so retained objects
//! cannot hide later duplicates forever.

pub(super) mod cursor;

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
        let mut walk = cursor::Walk::new(
            &backup,
            report.backup_cursor.take(),
            remaining_scan,
            deadline,
            home_device,
        );
        let result = (|| -> Result<(), JanitorError> {
            while let Some(path) = walk.next(report) {
                let Ok(relative) = path.strip_prefix(&backup) else {
                    report.skip_backup_twins("escapes_root", 1);
                    continue;
                };
                if namespace.trim().is_empty() && !relative.starts_with("ecosystem") {
                    report.skip_backup_twins("namespace_unconfigured", 1);
                    continue;
                }
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
        })();
        report.backup_cursor = walk.checkpoint();
        result
    };
    if let Err(exc) = body(report) {
        report.add_error(CLEANER, &exc);
    }
}
