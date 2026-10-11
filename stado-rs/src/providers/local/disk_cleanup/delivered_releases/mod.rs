//! The copies release delivery leaves under `~/.stado/releases`.
//!
//! Every delivery stages the release it installs at
//! `~/.stado/releases/<product>/<version>/<platform>/` — the attestation copy
//! `stado release version show` byte-compares the installed binary against, and
//! the retained archive — and nothing removed the earlier ones. A Linux
//! builder carried gigabytes of them under `/root/.stado/releases` while
//! every other cleaner reported nothing to take, stayed above the disk-full
//! threshold and refused release builds. The same sweep existed
//! only as `stado space reclaim`'s `delivery_leftovers` stage, run by hand.
//!
//! Kept for each product: the version its installed coordinate names
//! (`~/.stado/bin/<product>.release-version`), the version whose staged copy
//! is byte for byte the installed `~/.stado/bin/<product>` (deleting that one
//! turns a delivered host into an unattested one), and the newest version by
//! modification time, as the reclaim stage keeps it. Every other version
//! directory goes under the disk-full rule.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "delivered_releases";

/// Where delivery stages the releases it installs, relative to `$HOME`.
pub const DELIVERED_ROOT: &str = ".stado/releases";

/// Remove every delivered release version that no installed coordinate,
/// installed binary or newest delivery still needs.
pub fn scan_delivered_releases(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    let mut skipped: Vec<&'static str> = Vec::new();
    let root = home.join(DELIVERED_ROOT);
    let products: Vec<PathBuf> = match std::fs::read_dir(&root) {
        Ok(entries) => entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect(),
        Err(_) => Vec::new(),
    };
    for product in products {
        let name = product
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let Some(name) = name else {
            skipped.push("unnamed_product");
            continue;
        };
        let versions = version_directories(&product, &mut skipped);
        // What the host still needs could not be read: nothing of this
        // product is removed, and the cause is reported.
        let kept = match kept_versions(home, &name, &versions) {
            Ok(kept) => kept,
            Err(cause) => {
                record.scanned_items += versions.len() as i64;
                skipped.extend(versions.keys().map(|_| "installed_release_unreadable"));
                report.add_error(CLEANER, &super::JanitorError::os(&cause));
                continue;
            }
        };
        record.scanned_items += versions.len() as i64;
        let candidates: Vec<&PathBuf> = versions
            .iter()
            .filter(|(version, _)| !kept.contains(*version))
            .map(|(_, (path, _))| path)
            .collect();
        skipped.extend(
            versions
                .keys()
                .filter(|version| kept.contains(*version))
                .map(|_| "installed_or_newest"),
        );
        record.eligible_items += candidates.len() as i64;
        let sizes: Vec<(&PathBuf, i64)> = candidates
            .into_iter()
            .map(|path| (path, tree_bytes(path)))
            .collect();
        record.expected_bytes += sizes.iter().map(|(_, bytes)| bytes).sum::<i64>();
        if !enforcing {
            continue;
        }
        let mut freed: Vec<i64> = Vec::new();
        for (path, bytes) in sizes {
            match std::fs::remove_dir_all(path) {
                Ok(()) => freed.push(bytes),
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
        record.deleted_items += freed.len() as i64;
        record.actual_free_delta_bytes += freed.iter().sum::<i64>();
    }
    for reason in skipped.iter().collect::<BTreeSet<_>>() {
        *record.skipped.entry((*reason).to_string()).or_default() +=
            skipped.iter().filter(|seen| *seen == reason).count() as i64;
    }
    report.delivered_releases = record;
}

/// Each version directory of one product, with its modification time. A
/// symbolic link is never followed or removed.
fn version_directories(
    product: &Path,
    skipped: &mut Vec<&'static str>,
) -> BTreeMap<String, (PathBuf, SystemTime)> {
    let mut versions = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(product) else {
        skipped.push("directory_unreadable");
        return versions;
    };
    for entry in entries.flatten() {
        let Ok(info) = std::fs::symlink_metadata(entry.path()) else {
            skipped.push("unreadable");
            continue;
        };
        if !info.is_dir() {
            continue;
        }
        let Ok(modified) = info.modified() else {
            skipped.push("modification_time_unreadable");
            continue;
        };
        versions.insert(
            entry.file_name().to_string_lossy().into_owned(),
            (entry.path(), modified),
        );
    }
    versions
}

/// The versions of `product` this host still needs. An installed coordinate,
/// installed binary or staged copy that exists and cannot be read is an
/// error naming it: read as absent, the version the host runs was removed.
fn kept_versions(
    home: &Path,
    product: &str,
    versions: &BTreeMap<String, (PathBuf, SystemTime)>,
) -> Result<BTreeSet<String>, String> {
    let mut kept = BTreeSet::new();
    let bin = home.join(".stado").join("bin");
    let coordinate_path = bin.join(format!("{product}.release-version"));
    match std::fs::read_to_string(&coordinate_path) {
        Ok(coordinate) => {
            if let Some(version) = coordinate.split_whitespace().next() {
                kept.insert(version.to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot read {}: {error}",
                coordinate_path.display()
            ))
        }
    }
    let installed_path = bin.join(product);
    let installed = match std::fs::read(&installed_path) {
        Ok(installed) => Some(installed),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot read {}: {error}", installed_path.display())),
    };
    if let Some(installed) = installed {
        for (version, (path, _)) in versions {
            let platforms = std::fs::read_dir(path)
                .map_err(|error| format!("cannot list {}: {error}", path.display()))?;
            for platform in platforms {
                let platform =
                    platform.map_err(|error| format!("cannot list {}: {error}", path.display()))?;
                let copy_path = platform.path().join(product);
                let attested = match std::fs::read(&copy_path) {
                    Ok(copy) => copy == installed,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => {
                        return Err(format!("cannot read {}: {error}", copy_path.display()))
                    }
                };
                if attested {
                    kept.insert(version.clone());
                }
            }
        }
    }
    if let Some((newest, _)) = versions.iter().max_by_key(|(_, (_, modified))| *modified) {
        kept.insert(newest.clone());
    }
    Ok(kept)
}

/// Bytes under a directory, counting plain files only. A directory, entry or
/// file that cannot be read is left out of the count and named on stderr, so
/// a reclaim that reports fewer bytes than it freed says why.
fn tree_bytes(root: &Path) -> i64 {
    let mut sizes: Vec<u64> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!(
                    "delivered releases: {} is not counted: {error}",
                    directory.display()
                );
                continue;
            }
        };
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    eprintln!(
                        "delivered releases: an entry of {} is not counted: {error}",
                        directory.display()
                    );
                    continue;
                }
            };
            match std::fs::symlink_metadata(&path) {
                Ok(info) if info.is_dir() => stack.push(path),
                Ok(info) if info.is_file() => sizes.push(info.len()),
                Ok(_) => {}
                Err(error) => eprintln!(
                    "delivered releases: {} is not counted: {error}",
                    path.display()
                ),
            }
        }
    }
    sizes.iter().sum::<u64>() as i64
}
