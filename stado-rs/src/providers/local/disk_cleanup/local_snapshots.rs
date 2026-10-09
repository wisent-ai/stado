//! Local Time Machine snapshots.
//!
//! macOS keeps a local APFS snapshot of the volume every hour. A snapshot
//! pins every block the volume held when it was taken, so deleting a file
//! frees nothing while a snapshot still references it. A pass can therefore
//! remove every tagged build tree it finds and leave `df` exactly where it
//! started.
//!
//! This is the second half, run by the pass itself after every other
//! cleaner: under the disk-full rule it deletes every Time Machine local
//! snapshot, so the blocks the other cleaners released become free space.
//!
//! `com.apple.os.update-*` snapshots are the operating system's recovery
//! state, not backups; they are retained and counted, never deleted.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use super::free_bytes;
use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "local_snapshots";

/// The reader and the remover. Fixed absolute paths, the way every other
/// cleaner names the tools it runs.
const TMUTIL: &str = "/usr/bin/tmutil";

/// What `tmutil listlocalsnapshots /` prefixes every Time Machine snapshot
/// with, and the prefix the operating system's own update snapshots carry.
const SNAPSHOT_PREFIX: &str = "com.apple.TimeMachine.";
const OS_UPDATE_PREFIX: &str = "com.apple.os.update-";
/// What the same listing appends to every snapshot name. `tmutil
/// deletelocalsnapshots` takes the timestamp alone and reads anything else
/// as a volume, so a name handed over whole is refused with
/// `<name> is not a valid disk` — six times an hour on this fleet's macs,
/// with nothing deleted and the host left under its watermark.
const SNAPSHOT_SUFFIX: &str = ".local";
/// `YYYY-MM-DD-HHMMSS`, the shape tmutil prints and accepts. A listing row
/// that does not carry it is counted and left alone rather than handed to
/// the remover as an argument nobody can predict.
const STAMP_LENGTH: usize = "0000-00-00-000000".len();

/// The timestamp `tmutil deletelocalsnapshots` takes, read out of one
/// listing row. `None` for a row whose remainder is not a timestamp.
fn stamp_of(line: &str) -> Option<&str> {
    let stamp = line
        .strip_prefix(SNAPSHOT_PREFIX)?
        .trim_end_matches(SNAPSHOT_SUFFIX);
    let shaped = stamp.len() == STAMP_LENGTH
        && stamp
            .chars()
            .enumerate()
            .all(|(place, letter)| match place {
                4 | 7 | 10 => letter == '-',
                _ => letter.is_ascii_digit(),
            });
    shaped.then_some(stamp)
}

/// Delete every Time Machine local snapshot of this volume.
///
/// Returns without touching anything when the host is not macOS or when the
/// pass is planning rather than enforcing.
pub fn delete_all(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    let before = free_bytes(home).ok();
    if !cfg!(target_os = "macos") {
        bump(&mut record.skipped, "host_is_not_macos");
        report.local_snapshots = record;
        return;
    }
    let listed = match crate::wait::output(Command::new(TMUTIL).args(["listlocalsnapshots", "/"])) {
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
        let Some(stamp) = stamp_of(line) else {
            if line.starts_with(OS_UPDATE_PREFIX) {
                record.scanned_items += 1;
                bump(&mut record.skipped, "operating_system_recovery_state");
            } else if line.starts_with(SNAPSHOT_PREFIX) {
                record.scanned_items += 1;
                bump(&mut record.skipped, "name_carries_no_timestamp");
            }
            continue;
        };
        record.scanned_items += 1;
        record.eligible_items += 1;
        if !enforcing {
            continue;
        }
        match crate::wait::output(Command::new(TMUTIL).args(["deletelocalsnapshots", stamp])) {
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
