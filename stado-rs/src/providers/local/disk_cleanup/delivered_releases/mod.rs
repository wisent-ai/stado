//! The copies release delivery leaves under `~/.stado/releases`.
//!
//! Every delivery stages the release it installs at
//! `~/.stado/releases/<product>/<version>/<platform>/` — the attestation copy
//! `stado release version show` byte-compares the installed binary against, and
//! the retained archive — and nothing removed the earlier ones. A Linux
//! builder carried 7.9 GB of them under `/root/.stado/releases` while every
//! other cleaner reported nothing to take, stayed above the disk-full
//! threshold and refused release builds (f036eefe). The same sweep existed
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
        let kept = kept_versions(home, &name, &versions);
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

/// The versions of `product` this host still needs.
fn kept_versions(
    home: &Path,
    product: &str,
    versions: &BTreeMap<String, (PathBuf, SystemTime)>,
) -> BTreeSet<String> {
    let mut kept = BTreeSet::new();
    let bin = home.join(".stado").join("bin");
    if let Ok(coordinate) = std::fs::read_to_string(bin.join(format!("{product}.release-version")))
    {
        if let Some(version) = coordinate.split_whitespace().next() {
            kept.insert(version.to_string());
        }
    }
    if let Ok(installed) = std::fs::read(bin.join(product)) {
        for (version, (path, _)) in versions {
            let Ok(platforms) = std::fs::read_dir(path) else {
                continue;
            };
            let attested = platforms.flatten().any(|platform| {
                std::fs::read(platform.path().join(product)).is_ok_and(|copy| copy == installed)
            });
            if attested {
                kept.insert(version.clone());
            }
        }
    }
    if let Some((newest, _)) = versions.iter().max_by_key(|(_, (_, modified))| *modified) {
        kept.insert(newest.clone());
    }
    kept
}

/// Bytes under a directory, counting plain files only.
fn tree_bytes(root: &Path) -> i64 {
    let mut sizes: Vec<u64> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(info) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if info.is_dir() {
                stack.push(entry.path());
            } else if info.is_file() {
                sizes.push(info.len());
            }
        }
    }
    sizes.iter().sum::<u64>() as i64
}
