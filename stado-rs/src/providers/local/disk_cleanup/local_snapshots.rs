//! Local Time Machine snapshots, thinned to the declared target.
//!
//! macOS keeps a local APFS snapshot of the volume every hour. A snapshot
//! pins every block the volume held when it was taken, so deleting a file
//! frees nothing while a snapshot still references it — and the janitor's own
//! accounting says exactly that: on a fleet Mac on 2026-09-21 a pass
//! removed 54 tagged build trees and `df` moved from 12.4 GiB free to 12.4
//! GiB free. Thinning the eleven local snapshots on the same volume, through
//! the declared `local_apfs_snapshots` reclamation stage, moved it to 35.8
//! GiB. Both halves were needed and only the first one was automatic, so the
//! host went back under its watermark within the hour, published
//! `disk_pressure_active`, and refused every release build again.
//!
//! This is the second half, run by the pass itself. It is bounded by the same
//! declared target the rest of the janitor is measured against: snapshots are
//! deleted oldest first and the loop stops as soon as the volume is at its
//! target, so a host with headroom keeps its whole backup history.
//!
//! `com.apple.os.update-*` snapshots are the operating system's recovery
//! state, not backups; they are retained and counted, never deleted.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use super::janitor::state::report::{CleanerReport, CleanupReport};
use super::{free_bytes, GIB};
use crate::targets::DiskCleanupPolicy;

/// The key under `targets[].disk_cleanup.cleaners`.
pub const CLEANER: &str = "local_snapshots";

/// The reader and the remover. Fixed absolute paths, the way every other
/// cleaner names the tools it runs.
const TMUTIL: &str = "/usr/bin/tmutil";

/// What `tmutil listlocalsnapshots /` prefixes every Time Machine snapshot
/// with, and the prefix the operating system's own update snapshots carry.
const SNAPSHOT_PREFIX: &str = "com.apple.TimeMachine.";
const OS_UPDATE_PREFIX: &str = "com.apple.os.update-";

/// Thin this volume's local snapshots until it reaches the declared target.
///
/// Returns without touching anything when the host is not macOS, when the
/// policy does not declare this cleaner, when the pass is planning rather
/// than enforcing, or when the volume is already at its target.
pub fn thin_to_target(
    home: &Path,
    policy: &DiskCleanupPolicy,
    enforcing: bool,
    report: &mut CleanupReport,
) {
    if !policy.cleaners.contains_key(CLEANER) {
        return;
    }
    let mut record = CleanerReport::default();
    let target_bytes = policy.target_free_gb.saturating_mul(GIB);
    let before = free_bytes(home).ok();
    if !cfg!(target_os = "macos") {
        bump(&mut record.skipped, "host_is_not_macos");
        report.local_snapshots = record;
        return;
    }
    let listed = match Command::new(TMUTIL)
        .args(["listlocalsnapshots", "/"])
        .output()
    {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        Ok(output) => {
            bump(&mut record.skipped, "listing_refused");
            report.add_error(
                CLEANER,
                &super::JanitorError::os(&format!(
                    "tmutil listlocalsnapshots refused: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
            );
            report.local_snapshots = record;
            return;
        }
        Err(error) => {
            bump(&mut record.skipped, "tmutil_unavailable");
            report.add_error(
                CLEANER,
                &super::JanitorError::os(&format!("tmutil could not be run: {error}")),
            );
            report.local_snapshots = record;
            return;
        }
    };

    for line in listed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some(stamp) = line.strip_prefix(SNAPSHOT_PREFIX) else {
            if line.starts_with(OS_UPDATE_PREFIX) {
                record.scanned_items += 1;
                bump(&mut record.skipped, "operating_system_recovery_state");
            }
            continue;
        };
        record.scanned_items += 1;
        // The target is the whole bound: a volume already at it keeps every
        // snapshot it holds, which is what a backup history is for.
        match free_bytes(home) {
            Ok(free) if free >= target_bytes => {
                bump(&mut record.skipped, "volume_at_declared_target");
                continue;
            }
            Ok(_) => {}
            Err(_) => {
                bump(&mut record.skipped, "free_space_unreadable");
                continue;
            }
        }
        record.eligible_items += 1;
        if !enforcing {
            continue;
        }
        match Command::new(TMUTIL)
            .args(["deletelocalsnapshots", stamp])
            .output()
        {
            Ok(output) if output.status.success() => record.deleted_items += 1,
            Ok(output) => {
                bump(&mut record.skipped, "deletion_refused");
                report.add_error(
                    CLEANER,
                    &super::JanitorError::os(&format!(
                        "tmutil deletelocalsnapshots {stamp} refused: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    )),
                );
            }
            Err(error) => {
                bump(&mut record.skipped, "deletion_failed");
                report.add_error(
                    CLEANER,
                    &super::JanitorError::os(&format!("tmutil could not be run: {error}")),
                );
            }
        }
    }

    // macOS publishes no per-snapshot size, so the only honest figure for
    // what this cleaner recovered is the volume's own measurement across it.
    if let (Some(before), Ok(after)) = (before, free_bytes(home)) {
        record.actual_free_delta_bytes = after.saturating_sub(before);
    }
    report.local_snapshots = record;
}

fn bump(counts: &mut BTreeMap<String, i64>, reason: &str) {
    *counts.entry(reason.to_string()).or_default() += 1;
}
