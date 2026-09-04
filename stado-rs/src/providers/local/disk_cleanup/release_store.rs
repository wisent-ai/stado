//! Retention for the immutable release objects a host's local store holds.
//!
//! Every `stado release submit` publishes a product version into
//! `ecosystem/releases/<product>/<version>/` of the canonical store, and the
//! objects are immutable by contract: nothing ever rewrites or deletes one
//! through the object API. On the host that carries the store's files that
//! contract had no counterpart. Measured on `charless-mac-mini` on
//! 2026-09-04: `local-storage/ecosystem/releases/stado` held 84 versions,
//! 21.6 GiB, while the disk sat under the janitor's 15 GiB low watermark with
//! every declared cleaner reporting zero — and the same release loop that
//! filled it kept publishing 0.6 GiB stado releases into it, each one failing
//! to land because the host would not claim work under disk pressure. The
//! loop that needed the disk was the loop that consumed it, and nothing in the
//! janitor could name what it was looking at.
//!
//! What a release version is still for, and therefore what this cleaner keeps:
//!
//! - a version a host is running, rolled back from, or cutting over to: named
//!   by `active`, `previous` or `candidate` in any `<state_dir>/<product>.json`
//!   this host's release agent writes;
//! - a version a pipeline run still names, because a delivery job may fetch
//!   it: every run record under `runs/release-pipeline/` whose state is not
//!   terminal, and every run younger than the policy's `min_age_seconds`
//!   regardless of state, so a just-completed run can still be redelivered;
//! - the newest `keep_newest` versions of each product, ordered by version
//!   number, as the rollback ladder an operator can still reach through
//!   `stado release rollback`.
//!
//! Everything else under a product's release prefix is a version no host runs,
//! no run names, and no operator can reach without republishing — and
//! republishing is exactly what the pipeline does from source on request. A
//! version directory is deleted whole, never one object of it: a release with
//! half its files is worse than none, because `SHA256SUMS` would still name
//! the missing ones and a delivery would fail after the download rather than
//! before it.
//!
//! Two things this cleaner refuses on purpose. It never touches a product that
//! has no state file and no run record on this host, because a store can hold
//! releases for a product this host does not serve and whose consumers it
//! cannot see; those are kept and reported as `product_not_served_here`. When
//! the policy omits `keep_newest`, the conservative default keeps the newest
//! three versions as a rollback ladder.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::targets::DiskCleanupPolicy;

use super::{euid, free_bytes, CleanupReport, JanitorError, GIB};

pub const CLEANER: &str = "release_store";

/// Where the local store keeps release objects, relative to `$HOME`. Same
/// default as `storage.local.path` plus the object namespace root.
pub const RELEASES_ROOT: &str = ".stado/local-storage/ecosystem/releases";

/// Where release pipeline runs live inside the store, relative to the
/// namespace directory of the store's own product namespace.
const RUNS_PREFIX: &str = "runs/release-pipeline";

/// The release agent's state directory, relative to `$HOME`, when the policy
/// names none. Same default as the release target policy.
pub const STATE_DIR: &str = ".stado/release-state";

/// How many newest versions per product survive with no other reason, when
/// the policy sets `keep_newest` without a number.
const DEFAULT_KEEP_NEWEST: usize = 3;

/// One product's versions on disk, with the bytes each one occupies.
#[derive(Debug, Default)]
struct ProductReleases {
    versions: BTreeMap<String, (PathBuf, i64)>,
}

/// Version strings ordered as numbers, newest last; a version that is not
/// dotted numbers sorts before every one that is, so it is never counted as
/// the newest of anything.
fn version_key(version: &str) -> (bool, Vec<u64>) {
    let parts: Option<Vec<u64>> = version
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect();
    match parts {
        Some(numbers) => (true, numbers),
        None => (false, Vec::new()),
    }
}

/// Bytes under a directory, counting plain files only.
fn tree_bytes(root: &Path) -> i64 {
    let mut total = 0i64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => {
                    total += entry.metadata().map(|m| m.len() as i64).unwrap_or_default();
                }
                _ => {}
            }
        }
    }
    total
}

/// The versions the release agent on this host still has a use for, per
/// product, read from every state file in `state_dir`.
fn host_pinned_versions(state_dir: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(state_dir) else {
        return pinned;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|e| e.to_str()) != Some("json") || stem.ends_with("-proxy") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let product = state
            .get("product")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(stem)
            .to_string();
        let versions = pinned.entry(product).or_default();
        for slot in ["active", "previous", "candidate"] {
            if let Some(version) = state
                .get(slot)
                .and_then(|record| record.get("version"))
                .and_then(serde_json::Value::as_str)
            {
                versions.insert(version.to_string());
            }
        }
        // A quarantined digest names a version an operator may still inspect.
        if let Some(quarantined) = state
            .get("quarantined")
            .and_then(serde_json::Value::as_object)
        {
            for record in quarantined.values() {
                if let Some(version) = record.get("version").and_then(serde_json::Value::as_str) {
                    versions.insert(version.to_string());
                }
            }
        }
    }
    pinned
}

/// The versions a pipeline run may still fetch, per product: every run that
/// is not terminal, and every run younger than `min_age_seconds`.
///
/// Release runs are stored as `<product>/<run-id>/run.json`. The bounded walk
/// also accepts the older `<run-id>/run.json` layout, but never follows links.
///
/// `ecosystem` is the store's namespace root. The runs live under the store's
/// product namespace, and that namespace is a storage binding this host may
/// not carry — a host bound to a local backend resolves it to the empty
/// string, which turned the runs path into `ecosystem//runs/…` and made every
/// run invisible; a `publishing` run's version was then deleted in the probe
/// that caught it. So every namespace directory is walked. A run this cleaner
/// cannot see is a version it would delete, and seeing all of them is the
/// safe direction.
fn run_pinned_versions(
    ecosystem: &Path,
    min_age_seconds: i64,
    now_epoch: i64,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut directories = Vec::new();
    if let Ok(namespaces) = std::fs::read_dir(ecosystem) {
        for namespace in namespaces.flatten() {
            if namespace
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false)
            {
                directories.push((namespace.path().join(RUNS_PREFIX), 0usize));
            }
        }
    }
    while let Some((directory, depth)) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < 2 {
                directories.push((entry.path(), depth + 1));
                continue;
            }
            if !kind.is_file() || entry.file_name() != "run.json" {
                continue;
            }
            let record = entry.path();
            let Ok(text) = std::fs::read_to_string(&record) else {
                continue;
            };
            let Ok(run) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let (Some(product), Some(version)) = (
                run.get("product").and_then(serde_json::Value::as_str),
                run.get("version").and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            let state = run
                .get("state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let terminal = matches!(state, "completed" | "failed" | "reconciled");
            let young = std::fs::metadata(&record)
                .map(|meta| now_epoch - meta.mtime() < min_age_seconds)
                .unwrap_or(true);
            if !terminal || young {
                pinned
                    .entry(product.to_string())
                    .or_default()
                    .insert(version.to_string());
            }
        }
    }
    pinned
}

/// Reclaim release versions nothing on this host still has a use for.
pub fn scan_release_store(
    home: &Path,
    policy: &DiskCleanupPolicy,
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
        let keep_newest = match configured.keep_newest {
            Some(keep) if keep > 0 => keep as usize,
            Some(_) => {
                report.skip_release_store("keep_newest_zero", 1);
                return Ok(());
            }
            None => DEFAULT_KEEP_NEWEST,
        };
        let releases = match &configured.root {
            Some(root) => crate::config_file::expand_tilde(root),
            None => home.join(RELEASES_ROOT),
        };
        if !releases.is_dir() {
            report.skip_release_store("root_absent", 1);
            return Ok(());
        }
        let state_dir = home.join(STATE_DIR);
        let ecosystem = releases
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".stado/local-storage/ecosystem"));
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        let host_pins = host_pinned_versions(&state_dir);
        let run_pins = run_pinned_versions(&ecosystem, configured.min_age_seconds, now_epoch);
        let home_device = std::fs::metadata(home)?.dev();

        // Inventory: every product directory, every version directory under it.
        let mut products: BTreeMap<String, ProductReleases> = BTreeMap::new();
        let mut scanned = 0i64;
        for product_entry in std::fs::read_dir(&releases)?.flatten() {
            let product_path = product_entry.path();
            let Ok(product_info) = std::fs::symlink_metadata(&product_path) else {
                report.skip_release_store("product_stat_failed", 1);
                continue;
            };
            if !product_info.is_dir()
                || product_info.uid() != euid()
                || product_info.dev() != home_device
            {
                report.skip_release_store("product_not_an_owned_directory", 1);
                continue;
            }
            let product = product_entry.file_name().to_string_lossy().to_string();
            let Ok(versions) = std::fs::read_dir(&product_path) else {
                report.skip_release_store("read_dir_failed", 1);
                continue;
            };
            for version_entry in versions.flatten() {
                if Instant::now() >= deadline {
                    report.caps.deadline = true;
                    report.skip_release_store("scan_deadline", 1);
                    break;
                }
                if scanned >= remaining_scan {
                    report.caps.scan = true;
                    report.skip_release_store("scan_cap", 1);
                    break;
                }
                scanned += 1;
                report.release_store.scanned_items += 1;
                let version_path = version_entry.path();
                let Ok(info) = std::fs::symlink_metadata(&version_path) else {
                    report.skip_release_store("stat_failed", 1);
                    continue;
                };
                if !info.is_dir() || info.uid() != euid() || info.dev() != home_device {
                    report.skip_release_store("not_an_owned_directory", 1);
                    continue;
                }
                let version = version_entry.file_name().to_string_lossy().to_string();
                let bytes = tree_bytes(&version_path);
                products
                    .entry(product.clone())
                    .or_default()
                    .versions
                    .insert(version, (version_path, bytes));
            }
        }

        let mut deleted_bytes = 0i64;
        for (product, inventory) in &products {
            let served_here = host_pins.contains_key(product) || run_pins.contains_key(product);
            if !served_here {
                report
                    .skip_release_store("product_not_served_here", inventory.versions.len() as i64);
                continue;
            }
            let mut ordered: Vec<&String> = inventory.versions.keys().collect();
            ordered.sort_by_key(|version| version_key(version));
            let newest: BTreeSet<&String> =
                ordered.iter().rev().take(keep_newest).copied().collect();
            let empty = BTreeSet::new();
            let by_host = host_pins.get(product).unwrap_or(&empty);
            let by_run = run_pins.get(product).unwrap_or(&empty);
            for version in &ordered {
                let (path, bytes) = &inventory.versions[*version];
                if by_host.contains(*version) {
                    report.skip_release_store("host_state_names_it", 1);
                    continue;
                }
                if by_run.contains(*version) {
                    report.skip_release_store("pipeline_run_names_it", 1);
                    continue;
                }
                if newest.contains(version) {
                    report.skip_release_store("newest_kept", 1);
                    continue;
                }
                report.release_store.eligible_items += 1;
                report.release_store.expected_bytes += bytes;
                if policy.mode != "enforce" {
                    continue;
                }
                if report.release_store.deleted_items >= policy.max_items_per_pass {
                    report.caps.items = true;
                    report.skip_release_store("item_cap", 1);
                    continue;
                }
                if deleted_bytes.saturating_add(*bytes) > policy.max_bytes_per_pass {
                    report.caps.bytes = true;
                    report.skip_release_store("byte_cap", 1);
                    continue;
                }
                if free_bytes(home)? >= policy.target_free_gb * GIB {
                    break;
                }
                let delete_attempt = (|| -> Result<i64, JanitorError> {
                    let current = std::fs::symlink_metadata(path)?;
                    if !current.is_dir() || current.uid() != euid() || current.dev() != home_device
                    {
                        return Err(JanitorError::os(
                            "release directory identity changed before deletion",
                        ));
                    }
                    let before = free_bytes(home)?;
                    std::fs::remove_dir_all(path)?;
                    Ok(free_bytes(home)? - before)
                })();
                match delete_attempt {
                    Ok(delta) => {
                        report.release_store.actual_free_delta_bytes += delta.max(0);
                        report.release_store.deleted_items += 1;
                        deleted_bytes += bytes;
                    }
                    Err(exc) => report.add_error(CLEANER, &exc),
                }
            }
        }
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error(CLEANER, &exc);
    }
}
