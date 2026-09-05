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
//! - a version any host in the registry DECLARES, through
//!   `targets[].managed_versions`, and any version an operator pins in a
//!   config file on this host through `release.version`. These two were the
//!   gap. On 2026-09-04 `stado/0.15.21/darwin-arm64` was published complete,
//!   read successfully through the public release route at 21:21Z, and
//!   answered `{"state":"absent"}` for every one of its objects by 21:50Z:
//!   this cleaner deleted it under disk pressure because the object-API
//!   host's `~/.stado/release-state` was empty and nothing else named it.
//!   `install-stado.sh`, `self_update.rs`, `deploy/host_release.rs` and
//!   `deploy/host_recovery_release.rs` all pin a version by
//!   `STADO_RELEASE_VERSION` / `release.version`, and a declaration in the
//!   registry is what `host declare-version` writes — neither was a pin here,
//!   so the fleet's own installers pinned versions this cleaner was free to
//!   remove. A version somebody has declared is not reclaimable scratch;
//! - a version a pipeline run still names, because a delivery job may fetch
//!   it: every run record under `runs/release-pipeline/` whose state is not
//!   terminal, and every run younger than the policy's `min_age_seconds`
//!   regardless of state, so a just-completed run can still be redelivered;
//! - the newest `keep_newest` versions of each product, ordered by version
//!   number, as the rollback ladder an operator can still reach through
//!   `stado release rollback`;
//! - the newest version that is actually INSTALLABLE — one carrying the full
//!   installer family for some platform: `<product>-v<version>-<platform>.tar.gz`,
//!   `release-manifest-<platform>.json` and `SHA256SUMS`. A newer coordinate
//!   is not a substitute for it. Every publisher claims
//!   `source-revision.json` create-only BEFORE any artifact
//!   (`release_control::RELEASE_REVISION_NAME`), so an interrupted publish
//!   leaves a version directory holding that one small file — and four such
//!   claims are exactly what filled the newest-three ladder on 2026-09-04
//!   while the last installable version fell off the bottom of it. A claim is
//!   not a release: counting one as the rollback ladder leaves a host with
//!   nothing to install and nothing to roll back to.
//!
//! Everything else under a product's release prefix is a version no host runs,
//! nobody declares, no run names, and no operator can reach without
//! republishing — and republishing is exactly what the pipeline does from
//! source on request. A version directory is deleted whole, never one object
//! of it: a release with half its files is worse than none, because
//! `SHA256SUMS` would still name the missing ones and a delivery would fail
//! after the download rather than before it. Every such deletion is logged at
//! `warn`, one line per version, naming the product, version, bytes and the
//! reasons it was not pinned: a whole release leaving a host under disk
//! pressure is the kind of reclaim an operator has to be able to read after
//! the fact, and this cleaner's counters said only how many.
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

use serde_json::Value;

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

/// Config files an operator's version pin can live in, relative to `$HOME`,
/// in the same order [`crate::config_file::CANDIDATES`] resolves them. The
/// pin is read from EVERY one of them rather than from the winning file:
/// this cleaner is not resolving configuration, it is asking whether any
/// declaration on this host still needs a version, and a shadowed file's
/// answer is as expensive to get wrong as the winner's.
const CONFIG_CANDIDATES: [&str; 3] = [
    ".config/stado/config.json",
    ".stado/config.json",
    "stado.config.json",
];

/// The dotted config key holding the exact release version an installer
/// consumes, as [`crate::config::stado_release_version`] reads it.
const CONFIG_VERSION_KEY: [&str; 2] = ["release", "version"];

/// The product a bare `release.version` pin belongs to. That key names no
/// product because Stado's own installers are its only readers.
const CONFIG_VERSION_PRODUCT: &str = "stado";

/// The signed manifest name of one platform's installable archive, as
/// `install-stado.sh`, `self_update.rs` and `deploy/host_release.rs` build it.
fn platform_manifest_name(platform: &str) -> String {
    format!("release-manifest-{platform}.json")
}

/// The archive name those same readers request.
fn platform_archive_name(product: &str, version: &str, platform: &str) -> String {
    format!("{product}-v{version}-{platform}.tar.gz")
}

/// The digest list published beside them.
const PLATFORM_SUMS_NAME: &str = "SHA256SUMS";

/// How many newest versions per product survive with no other reason, when
/// the policy sets `keep_newest` without a number.
const DEFAULT_KEEP_NEWEST: usize = 3;

/// One product's versions on disk, with the bytes each one occupies and which
/// of them carry a complete installer family.
#[derive(Debug, Default)]
struct ProductReleases {
    versions: BTreeMap<String, (PathBuf, i64)>,
    installable: BTreeSet<String>,
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

/// The versions any host in the registry DECLARES, per product, read from
/// every target's `managed_versions`.
///
/// Not just this host's target. A release version a host declares must
/// survive on whichever host carries the store, and the host that carries
/// the store is usually not the host that runs the binary: on 2026-09-04 the
/// store lived on `charless-mac-mini` while the declarations that needed
/// those bytes belonged to every other target in the fleet. Reading only the
/// local target's declaration would leave that gap exactly as it was.
///
/// The registry document is taken as `Value` rather than as parsed targets
/// because this cleaner must not fail closed on a target shape a newer
/// release added: an unreadable target contributes no pin, and a pin this
/// cleaner cannot see is a version it would delete.
pub fn declared_versions(registry: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Some(targets) = registry.get("targets").and_then(Value::as_array) else {
        return pinned;
    };
    for target in targets {
        let Some(declared) = target
            .get(crate::deploy::host_release::MANAGED_VERSIONS_KEY)
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (product, version) in declared {
            let Some(version) = version.as_str() else {
                continue;
            };
            let version = version.trim();
            if version.is_empty() {
                continue;
            }
            pinned
                .entry(product.clone())
                .or_default()
                .insert(version.to_string());
        }
    }
    pinned
}

/// The versions an operator pins in a config file on this host, per product,
/// from `release.version` in every candidate config path under `home`.
///
/// This is the pin `install-stado.sh` reads as `STADO_RELEASE_VERSION`, that
/// `deploy/host_recovery_release.rs` recovers a wedged host with, and that
/// `providers/local/version_check.rs` measures a host against. An environment
/// variable cannot be a pin here — it belongs to whichever process exported
/// it, not to the host — so the file is the durable half, and the file is
/// what this reads.
fn config_pinned_versions(home: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for candidate in CONFIG_CANDIDATES {
        let path = home.join(candidate);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let mut node = &document;
        for key in CONFIG_VERSION_KEY {
            match node.get(key) {
                Some(next) => node = next,
                None => {
                    node = &Value::Null;
                    break;
                }
            }
        }
        let Some(version) = node.as_str().map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        pinned
            .entry(CONFIG_VERSION_PRODUCT.to_string())
            .or_default()
            .insert(version.to_string());
    }
    pinned
}

/// Whether one version directory carries the full installer family for at
/// least one platform: the archive, its platform manifest, and `SHA256SUMS`.
///
/// All three, from one platform directory. Two of them is what an interrupted
/// publish leaves, and `install-stado.sh` fetches the manifest and the archive
/// and then verifies the archive against the manifest's digest, so a version
/// missing either is a version that fails after the download instead of
/// before it.
fn is_installable(version_path: &Path, product: &str, version: &str) -> bool {
    let Ok(platforms) = std::fs::read_dir(version_path) else {
        return false;
    };
    for platform_entry in platforms.flatten() {
        let platform_path = platform_entry.path();
        if !platform_path.is_dir() {
            continue;
        }
        let platform = platform_entry.file_name().to_string_lossy().to_string();
        let required = [
            platform_archive_name(product, version, &platform),
            platform_manifest_name(&platform),
            PLATFORM_SUMS_NAME.to_string(),
        ];
        if required
            .iter()
            .all(|name| platform_path.join(name).is_file())
        {
            return true;
        }
    }
    false
}

/// Why one version survives a pass, or `None` when nothing needs it.
///
/// The reason strings are the report's own skip keys, so the retention
/// decision and what an operator reads are one thing rather than two that can
/// drift apart.
pub(crate) type KeepReason = Option<&'static str>;

/// The retention decision for one product, from names alone.
///
/// Separated from the filesystem on purpose: what survives a pass is a policy
/// question, and it is answered here over sets so it can be tested exhaustively
/// without a disk under pressure. `versions` is every version directory found,
/// in any order.
pub(crate) fn retention_decision<'a>(
    versions: &'a [String],
    installable: &BTreeSet<String>,
    by_host: &BTreeSet<String>,
    by_declaration: &BTreeSet<String>,
    by_config: &BTreeSet<String>,
    by_run: &BTreeSet<String>,
    keep_newest: usize,
) -> Vec<(&'a str, KeepReason)> {
    let mut ordered: Vec<&'a str> = versions.iter().map(String::as_str).collect();
    ordered.sort_by_key(|version| version_key(version));
    let newest: BTreeSet<&str> = ordered.iter().rev().take(keep_newest).copied().collect();
    // The newest version that is genuinely installable, which the rollback
    // ladder above cannot be relied on to include: a run of newer
    // source-revision-only claims pushes it out of the newest `keep_newest`
    // while adding nothing a host can install.
    let newest_installable: Option<&str> = ordered
        .iter()
        .rev()
        .copied()
        .find(|version| installable.contains(*version));
    ordered
        .into_iter()
        .map(|version| {
            let reason = if by_host.contains(version) {
                Some("host_state_names_it")
            } else if by_declaration.contains(version) {
                Some("host_declares_it")
            } else if by_config.contains(version) {
                Some("config_pins_it")
            } else if by_run.contains(version) {
                Some("pipeline_run_names_it")
            } else if newest.contains(version) {
                Some("newest_kept")
            } else if newest_installable == Some(version) {
                Some("newest_installable_kept")
            } else {
                None
            };
            (version, reason)
        })
        .collect()
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
///
/// `declared_pins` is [`declared_versions`] over the canonical registry this
/// pass resolved its policy from. It is passed in rather than fetched here
/// because this function must not perform network I/O — and an empty map is
/// the correct value when the registry did not answer, since the pass then
/// has no policy either and deletes nothing.
pub fn scan_release_store(
    home: &Path,
    policy: &DiskCleanupPolicy,
    declared_pins: &BTreeMap<String, BTreeSet<String>>,
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
        let config_pins = config_pinned_versions(home);
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
                let installable = is_installable(&version_path, &product, &version);
                let inventory = products.entry(product.clone()).or_default();
                if installable {
                    inventory.installable.insert(version.clone());
                }
                inventory.versions.insert(version, (version_path, bytes));
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
            let empty = BTreeSet::new();
            let versions: Vec<String> = inventory.versions.keys().cloned().collect();
            let decisions = retention_decision(
                &versions,
                &inventory.installable,
                host_pins.get(product).unwrap_or(&empty),
                declared_pins.get(product).unwrap_or(&empty),
                config_pins.get(product).unwrap_or(&empty),
                run_pins.get(product).unwrap_or(&empty),
                keep_newest,
            );
            for (version, keep) in decisions {
                let (path, bytes) = &inventory.versions[version];
                if let Some(reason) = keep {
                    report.skip_release_store(reason, 1);
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
                        // One line per version, at `warn`, because a whole
                        // immutable release leaving a host is not routine
                        // reclaim: on 2026-09-04 a complete installable
                        // `stado` 0.15.21 was removed and the only trace was
                        // a counter reading `deleted_items`, so the loss was
                        // reconstructed from a 404 half an hour later rather
                        // than read from the log. It names what was removed
                        // and, since nothing pinned it, which pins were
                        // consulted and came back empty.
                        tracing::warn!(
                            cleaner = CLEANER,
                            product = product.as_str(),
                            version,
                            bytes = *bytes,
                            installable = inventory.installable.contains(version),
                            not_pinned_by =
                                "host_state, host_declaration, config_release_version, pipeline_run",
                            keep_newest,
                            "reclaimed a release version under disk pressure"
                        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn versions(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// The decision for one version, by name.
    fn verdict(decisions: &[(&str, KeepReason)], version: &str) -> KeepReason {
        decisions
            .iter()
            .find(|(name, _)| *name == version)
            .map(|(_, reason)| *reason)
            .expect("every version present is decided")
    }

    /// The 2026-09-04 shape: four newer coordinates carrying nothing but a
    /// `source-revision.json` claim, and the last version anybody can install
    /// sitting below the newest-three ladder.
    #[test]
    fn the_newest_installable_release_survives_a_ladder_full_of_claims() {
        let present = versions(&["0.15.21", "0.15.25", "0.15.26", "0.16.0", "0.16.1"]);
        let decisions = retention_decision(
            &present,
            &set(&["0.15.21"]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(
            verdict(&decisions, "0.15.21"),
            Some("newest_installable_kept"),
            "the only installable release must not fall off the bottom of the ladder"
        );
        assert_eq!(verdict(&decisions, "0.16.1"), Some("newest_kept"));
        assert_eq!(verdict(&decisions, "0.16.0"), Some("newest_kept"));
        assert_eq!(verdict(&decisions, "0.15.26"), Some("newest_kept"));
        assert_eq!(
            verdict(&decisions, "0.15.25"),
            None,
            "a claim outside the ladder is still reclaimable"
        );
    }

    #[test]
    fn a_version_a_host_declares_survives_below_the_ladder() {
        let present = versions(&["0.14.6", "0.15.21", "0.16.0", "0.16.1", "0.16.2"]);
        let decisions = retention_decision(
            &present,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &set(&["0.14.6"]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(verdict(&decisions, "0.14.6"), Some("host_declares_it"));
        assert_eq!(verdict(&decisions, "0.15.21"), None);
    }

    #[test]
    fn a_version_an_operator_pins_in_config_survives_below_the_ladder() {
        let present = versions(&["0.15.3", "0.16.0", "0.16.1", "0.16.2"]);
        let decisions = retention_decision(
            &present,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &set(&["0.15.3"]),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(verdict(&decisions, "0.15.3"), Some("config_pins_it"));
    }

    /// Precedence is reported, not just obeyed: an operator reading
    /// `host_state_names_it` must be able to trust that the host's own state
    /// is why the version is still there.
    #[test]
    fn the_reported_reason_is_the_strongest_one_that_applies() {
        let present = versions(&["1.0.0"]);
        let decisions = retention_decision(
            &present,
            &set(&["1.0.0"]),
            &set(&["1.0.0"]),
            &set(&["1.0.0"]),
            &set(&["1.0.0"]),
            &set(&["1.0.0"]),
            1,
        );
        assert_eq!(verdict(&decisions, "1.0.0"), Some("host_state_names_it"));

        let decisions = retention_decision(
            &present,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &set(&["1.0.0"]),
            0,
        );
        assert_eq!(verdict(&decisions, "1.0.0"), Some("pipeline_run_names_it"));
    }

    /// Only the newest installable version is kept for that reason. An
    /// installable ancestor is ordinary history: keeping every one of them
    /// would be a store that never reclaims, which is the state this cleaner
    /// exists to end.
    #[test]
    fn older_installable_versions_are_still_reclaimable() {
        let present = versions(&["0.13.0", "0.14.6", "0.15.21", "0.16.0", "0.16.1"]);
        let decisions = retention_decision(
            &present,
            &set(&["0.13.0", "0.14.6", "0.15.21"]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            2,
        );
        assert_eq!(
            verdict(&decisions, "0.15.21"),
            Some("newest_installable_kept")
        );
        assert_eq!(verdict(&decisions, "0.14.6"), None);
        assert_eq!(verdict(&decisions, "0.13.0"), None);
    }

    /// A version whose name is not dotted numbers sorts below every one that
    /// is, so it must never be counted as the newest of anything — including
    /// the newest installable one.
    #[test]
    fn an_unparseable_version_name_is_never_the_newest() {
        let present = versions(&["__release_preflight__", "0.1.0"]);
        let decisions = retention_decision(
            &present,
            &set(&["__release_preflight__", "0.1.0"]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            1,
        );
        assert_eq!(verdict(&decisions, "0.1.0"), Some("newest_kept"));
        assert_eq!(verdict(&decisions, "__release_preflight__"), None);
    }

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn installability_needs_all_three_files_from_one_platform() {
        let home = tempfile::tempdir().unwrap();
        let complete = home.path().join("0.15.21");
        write(
            &complete.join("darwin-arm64/stado-v0.15.21-darwin-arm64.tar.gz"),
            b"archive",
        );
        write(
            &complete.join("darwin-arm64/release-manifest-darwin-arm64.json"),
            b"{}",
        );
        write(&complete.join("darwin-arm64/SHA256SUMS"), b"sums");
        assert!(is_installable(&complete, "stado", "0.15.21"));

        // The interrupted publish: the create-only claim and nothing else.
        let claim = home.path().join("0.16.1");
        write(&claim.join("darwin-arm64/source-revision.json"), b"{}");
        assert!(!is_installable(&claim, "stado", "0.16.1"));

        // Archive and sums but no platform manifest: `install-stado.sh` reads
        // the manifest first and verifies the archive against its digest, so
        // this is a download that fails after the bytes, not a release.
        let partial = home.path().join("0.16.0");
        write(
            &partial.join("darwin-arm64/stado-v0.16.0-darwin-arm64.tar.gz"),
            b"archive",
        );
        write(&partial.join("darwin-arm64/SHA256SUMS"), b"sums");
        assert!(!is_installable(&partial, "stado", "0.16.0"));

        // The three files exist, but split across two platforms: neither
        // platform is installable, and a caller asks for one platform.
        let split = home.path().join("0.16.2");
        write(
            &split.join("darwin-arm64/stado-v0.16.2-darwin-arm64.tar.gz"),
            b"archive",
        );
        write(
            &split.join("linux-amd64/release-manifest-linux-amd64.json"),
            b"{}",
        );
        write(&split.join("linux-amd64/SHA256SUMS"), b"sums");
        assert!(!is_installable(&split, "stado", "0.16.2"));

        // A version whose archive is named for another version is not this
        // version's release, which is what an unfinalised multipart upload
        // directory looks like from here.
        let mismatched = home.path().join("0.16.3");
        write(
            &mismatched.join("darwin-arm64/stado-v0.16.2-darwin-arm64.tar.gz"),
            b"archive",
        );
        write(
            &mismatched.join("darwin-arm64/release-manifest-darwin-arm64.json"),
            b"{}",
        );
        write(&mismatched.join("darwin-arm64/SHA256SUMS"), b"sums");
        assert!(!is_installable(&mismatched, "stado", "0.16.3"));
    }

    #[test]
    fn every_target_contributes_its_declared_versions() {
        let registry = json!({
            "targets": [
                {"name": "mini", "managed_versions": {"stado": "0.15.21", "skarbiec": "0.4.0"}},
                {"name": "macbook", "managed_versions": {"stado": "0.16.1"}},
                {"name": "blank", "managed_versions": {"stado": "   "}},
                {"name": "wrong-type", "managed_versions": {"stado": 15}},
                {"name": "undeclared"}
            ]
        });
        let declared = declared_versions(&registry);
        assert_eq!(declared["stado"], set(&["0.15.21", "0.16.1"]));
        assert_eq!(declared["skarbiec"], set(&["0.4.0"]));
    }

    #[test]
    fn a_registry_without_targets_pins_nothing_and_does_not_fail() {
        assert!(declared_versions(&json!({})).is_empty());
        assert!(declared_versions(&json!({"targets": "not an array"})).is_empty());
    }

    #[test]
    fn the_version_pin_is_read_from_every_config_candidate() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".config/stado/config.json"),
            br#"{"release": {"version": "0.15.3"}}"#,
        );
        // A file the resolver would never reach because the one above wins.
        // Its pin is still a version somebody wrote down on this host.
        write(
            &home.path().join(".stado/config.json"),
            br#"{"release": {"version": "0.14.6"}}"#,
        );
        write(&home.path().join("stado.config.json"), b"{ not json");
        let pinned = config_pinned_versions(home.path());
        assert_eq!(pinned[CONFIG_VERSION_PRODUCT], set(&["0.14.6", "0.15.3"]));
    }

    #[test]
    fn a_home_with_no_config_pins_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(config_pinned_versions(home.path()).is_empty());
        write(
            &home.path().join(".stado/config.json"),
            br#"{"release": {"platform": "darwin-arm64"}}"#,
        );
        assert!(config_pinned_versions(home.path()).is_empty());
    }
}
