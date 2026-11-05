//! Backup copies of installed binaries in `~/.stado/bin`.
//!
//! An install that replaces `~/.stado/bin/<binary>` keeps the copy it
//! replaced beside it, under a backup name (`<binary>.release-backup-<date>`,
//! `.bak-*`, `.pre-*`, `.<x.y.z>-backup-*`), and nothing in the janitor
//! removed them: a host carried twenty dated backups of a few binaries, two
//! and a half GiB, while the same sweep existed only as `stado space reclaim`'s
//! `delivery_leftovers` stage, run by hand.
//!
//! Under the disk-full rule this cleaner removes, for each installed binary,
//! every backup copy but the newest by modification time. It never touches
//! the binary, its `.previous` rollback target, its `.release-version`
//! coordinate, a symbolic link or a name of another shape; a backup whose
//! age cannot be read keeps every backup of that binary, since which one is
//! newest is then unknown.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "bin_backups";

/// Where installed binaries live, relative to `$HOME`.
pub const BIN_ROOT: &str = ".stado/bin";

/// The backup names installs give the copy they replace, as what follows
/// `<binary>.`: one list, read by [`backup_shape`] here and spliced into the
/// `delivery_leftovers` shell stage as its `case` patterns, so the janitor
/// and the hand-run stage never disagree about what a backup is. Each is a
/// literal prefix followed by at least one character.
pub const BACKUP_PREFIXES: &[&str] = &["release-backup-", "bak-", "pre-"];
/// The one shape with a version in front: `<x.y.z>-backup-<anything>`, each
/// of x, y and z starting in a digit.
pub const VERSIONED_BACKUP_INFIX: &str = "-backup-";

/// The same shapes as shell `case` patterns, `|`-joined, for the stage.
pub fn backup_case_patterns() -> String {
    BACKUP_PREFIXES
        .iter()
        .map(|prefix| format!("{prefix}?*"))
        .chain(std::iter::once(format!(
            "[0-9]*.[0-9]*.[0-9]*{VERSIONED_BACKUP_INFIX}?*"
        )))
        .collect::<Vec<_>>()
        .join("|")
}

/// Whether `suffix` (what follows `<binary>.`) is a backup name.
pub fn backup_shape(suffix: &str) -> bool {
    if BACKUP_PREFIXES.iter().any(|prefix| {
        suffix
            .strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty())
    }) {
        return true;
    }
    let Some((version, rest)) = suffix.split_once(VERSIONED_BACKUP_INFIX) else {
        return false;
    };
    let parts: Vec<&str> = version.splitn(3, '.').collect();
    !rest.is_empty()
        && parts.len() == 3
        && parts
            .iter()
            .all(|part| part.starts_with(|character: char| character.is_ascii_digit()))
}

/// Remove every backup of each installed binary but its newest.
pub fn scan_bin_backups(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    let mut skipped: Vec<&'static str> = Vec::new();
    let bin = home.join(BIN_ROOT);
    let entries = match std::fs::read_dir(&bin) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            report.bin_backups = record;
            return;
        }
        Err(error) => {
            report.add_error(
                CLEANER,
                &super::JanitorError::os(&format!("cannot list {}: {error}", bin.display())),
            );
            report.bin_backups = record;
            return;
        }
    };
    // Every plain file, by name; a link is never a candidate.
    let mut files: BTreeMap<String, (PathBuf, Option<SystemTime>, u64)> = BTreeMap::new();
    for entry in entries {
        let Ok(entry) = entry else {
            skipped.push("unreadable");
            continue;
        };
        let Ok(info) = std::fs::symlink_metadata(entry.path()) else {
            skipped.push("unreadable");
            continue;
        };
        if !info.is_file() {
            continue;
        }
        files.insert(
            entry.file_name().to_string_lossy().into_owned(),
            (entry.path(), info.modified().ok(), info.len()),
        );
    }
    // An installed binary is a name without a dot; its backups are the
    // files named `<binary>.<backup shape>`.
    let installed: Vec<&String> = files.keys().filter(|name| !name.contains('.')).collect();
    let mut eligible: Vec<(PathBuf, u64)> = Vec::new();
    for binary in installed {
        let prefix = format!("{binary}.");
        let backups: Vec<(&PathBuf, Option<SystemTime>, u64)> = files
            .iter()
            .filter_map(|(name, (path, modified, bytes))| {
                let suffix = name.strip_prefix(&prefix)?;
                (suffix != "previous" && backup_shape(suffix)).then_some((path, *modified, *bytes))
            })
            .collect();
        record.scanned_items += backups.len() as i64;
        if backups.is_empty() {
            continue;
        }
        if backups.iter().any(|(_, modified, _)| modified.is_none()) {
            skipped.extend(backups.iter().map(|_| "modification_time_unreadable"));
            continue;
        }
        let newest = backups
            .iter()
            .max_by_key(|(_, modified, _)| *modified)
            .map(|(path, _, _)| (*path).clone());
        for (path, _, bytes) in backups {
            if Some(path) == newest.as_ref() {
                skipped.push("newest_backup");
            } else {
                eligible.push((path.clone(), bytes));
            }
        }
    }
    record.eligible_items = eligible.len() as i64;
    record.expected_bytes = eligible.iter().map(|(_, bytes)| *bytes as i64).sum();
    if enforcing {
        for (path, bytes) in eligible {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    record.deleted_items += 1;
                    record.actual_free_delta_bytes += bytes as i64;
                }
                Err(error) => {
                    skipped.push("deletion_refused");
                    report.add_error(
                        CLEANER,
                        &super::JanitorError::os(&format!(
                            "cannot remove {}: {error}",
                            path.display()
                        )),
                    );
                }
            }
        }
    }
    for reason in skipped.iter().collect::<BTreeSet<_>>() {
        *record.skipped.entry((*reason).to_string()).or_default() +=
            skipped.iter().filter(|seen| *seen == reason).count() as i64;
    }
    report.bin_backups = record;
}
