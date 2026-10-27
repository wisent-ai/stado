//! Local Time Machine snapshots.
//!
//! macOS keeps a local APFS snapshot of the volume every hour. A snapshot
//! pins every block the volume held when it was taken, so deleting a file
//! frees nothing while a snapshot still references it. A pass can therefore
//! remove every tagged build tree it finds and leave `df` exactly where it
//! started.
//!
//! This is the second half, run by the pass itself after every other
//! cleaner: under the disk-full rule it deletes the Time Machine local
//! snapshots of the volume the pass's home lives on, and of no other volume,
//! so the blocks the other cleaners released become free space. The
//! deletion names that volume to `tmutil deletelocalsnapshots`; the dated
//! form of the same command deletes the snapshots of that date on every
//! mounted disk, so it is never used. A home on its own disk image, as every
//! test journey uses, has no snapshots, so a journey never reaches the
//! machine's own backups. The report's `local_snapshot_volume` names the
//! volume that was read and cleaned.
//!
//! `com.apple.os.update-*` snapshots are the operating system's recovery
//! state, not backups; they are counted and left alone, and the volume form
//! of the command deletes only Time Machine snapshots.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::free_bytes;
use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "local_snapshots";

/// The reader and the remover. Fixed absolute paths, the way every other
/// cleaner names the tools it runs.
const TMUTIL: &str = "/usr/bin/tmutil";
/// The reader of the mount point a path lives on.
const DF: &str = "/bin/df";

/// What `tmutil listlocalsnapshots` prefixes every Time Machine snapshot
/// with, and the prefix the operating system's own update snapshots carry.
const SNAPSHOT_PREFIX: &str = "com.apple.TimeMachine.";
const OS_UPDATE_PREFIX: &str = "com.apple.os.update-";

/// The mount point of the volume `home` lives on, as `df -P` names it in
/// its one volume row (the fields after the capacity percentage, which may
/// themselves hold spaces). A test's isolated home on its own disk image
/// resolves to that image, and a home under `/Users` to the data volume,
/// never to another volume.
fn volume_of(home: &Path) -> Result<PathBuf, String> {
    let output = crate::wait::output(Command::new(DF).arg("-P").arg(home))
        .map_err(|error| format!("df could not be run: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        return Err(format!(
            "df -P {} refused: {}",
            home.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let row = text
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .ok_or_else(|| format!("df -P {} printed nothing", home.display()))?;
    let fields: Vec<&str> = row.split_whitespace().collect();
    let capacity = fields
        .iter()
        .rposition(|field| field.ends_with('%'))
        .ok_or_else(|| {
            format!(
                "df -P {} printed a row without a capacity: {row}",
                home.display()
            )
        })?;
    let (_, from_capacity) = fields.split_at(capacity);
    let mount = match from_capacity.split_first() {
        Some((_, mount)) => mount.join(" "),
        None => String::new(),
    };
    if !mount.starts_with('/') {
        return Err(format!(
            "df -P {} printed no mount point: {row}",
            home.display()
        ));
    }
    Ok(PathBuf::from(mount))
}

/// File `error` under this cleaner, which touched nothing, and leave the
/// report with what it read.
fn refused(report: &mut CleanupReport, record: CleanerReport, error: String) {
    report.add_error(CLEANER, &super::JanitorError::os(&error));
    report.local_snapshots = record;
}

/// Delete every Time Machine local snapshot of the volume `home` lives on,
/// and of no other volume; the report names that volume.
///
/// Returns without touching anything when the host is not macOS or when the
/// pass is planning rather than enforcing.
pub fn delete_all(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    if !cfg!(target_os = "macos") {
        record
            .skipped
            .insert("host_is_not_macos".to_string(), Default::default());
        report.local_snapshots = record;
        return;
    }
    let volume = match volume_of(home) {
        Ok(volume) => volume,
        Err(error) => {
            let error = format!(
                "the volume of {} could not be read, so no snapshot is touched: {error}",
                home.display()
            );
            return refused(report, record, error);
        }
    };
    report.local_snapshot_volume = Some(volume.display().to_string());
    let mut listing = Command::new(TMUTIL);
    listing.arg("listlocalsnapshots").arg(&volume);
    let listed = match crate::wait::output(&mut listing) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).to_string()
        }
        Ok(output) => {
            let error = format!(
                "tmutil listlocalsnapshots {} refused, so no snapshot is touched: {}",
                volume.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return refused(report, record, error);
        }
        Err(error) => {
            let error = format!("tmutil could not be run, so no snapshot is touched: {error}");
            return refused(report, record, error);
        }
    };

    let rows: Vec<&str> = listed.lines().map(str::trim).collect();
    let backups: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|row| row.starts_with(SNAPSHOT_PREFIX))
        .collect();
    let recovery: Vec<&str> = rows
        .iter()
        .copied()
        .filter(|row| row.starts_with(OS_UPDATE_PREFIX))
        .collect();
    let backup_count = backups.len() as i64;
    record.scanned_items = backup_count + recovery.len() as i64;
    record.eligible_items = backup_count;
    if !recovery.is_empty() {
        record.skipped.insert(
            "operating_system_recovery_state".to_string(),
            recovery.len() as i64,
        );
    }
    if !enforcing || backups.is_empty() {
        report.local_snapshots = record;
        return;
    }
    let before = free_bytes(home);
    let mut deletion = Command::new(TMUTIL);
    deletion.arg("deletelocalsnapshots").arg(&volume);
    match crate::wait::output(&mut deletion) {
        Ok(output) if output.status.success() => record.deleted_items = backup_count,
        Ok(output) => {
            let error = format!(
                "tmutil deletelocalsnapshots {} refused: {}",
                volume.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            record
                .skipped
                .insert("deletion_refused".to_string(), backup_count);
            report.add_error(CLEANER, &super::JanitorError::os(&error));
        }
        Err(error) => {
            record
                .skipped
                .insert("deletion_failed".to_string(), backup_count);
            report.add_error(
                CLEANER,
                &super::JanitorError::os(&format!("tmutil could not be run: {error}")),
            );
        }
    }

    // macOS publishes no per-snapshot size, so the only honest figure for
    // what this cleaner recovered is the volume's own measurement across it.
    match (before, free_bytes(home)) {
        (Ok(before), Ok(after)) => record.actual_free_delta_bytes = after.saturating_sub(before),
        (Err(error), _) | (_, Err(error)) => report.add_error(CLEANER, &error),
    }
    report.local_snapshots = record;
}
