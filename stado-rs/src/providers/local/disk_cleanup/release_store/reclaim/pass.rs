//! One pass of the release-store cleaner: the inventory walk, the decision
//! applied to every version it found, and the report it writes.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;

use crate::providers::local::disk_cleanup::release_store::decision::retention_decision;
use crate::providers::local::disk_cleanup::release_store::inventory::families::complete_families;
use crate::providers::local::disk_cleanup::release_store::inventory::pins::{
    config_pinned_versions, host_pinned_versions,
};
use crate::providers::local::disk_cleanup::release_store::inventory::runs::run_retention_evidence;
use crate::providers::local::disk_cleanup::release_store::inventory::tree_bytes;
use crate::providers::local::disk_cleanup::release_store::{
    family_key, ProductReleases, ReleaseFamily, CLEANER, DEFAULT_KEEP_NEWEST, RELEASES_ROOT,
    STATE_DIR,
};
use crate::providers::local::disk_cleanup::{euid, free_bytes, CleanupReport, JanitorError, GIB};
use crate::targets::DiskCleanupPolicy;

use super::{remove_release_payloads, source_revision};

/// Reclaim release versions nothing on this host still has a use for.
///
/// `declared_pins` is [`declared_versions`](super::super::declared_versions)
/// over the canonical registry this
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
        let run_evidence =
            run_retention_evidence(&ecosystem, configured.min_age_seconds, now_epoch)?;
        let run_pins = &run_evidence.pinned;
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
                let complete = complete_families(&version_path, &product, &version);
                let inventory = products.entry(product.clone()).or_default();
                if !complete.is_empty() {
                    inventory.complete.insert(version.clone(), complete);
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
                &inventory.complete,
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
                let Some(claim) = source_revision(path, product, version) else {
                    report.skip_release_store("source_identity_unverified", 1);
                    continue;
                };
                let finished = run_evidence
                    .finished
                    .get(product)
                    .and_then(|versions| versions.get(version))
                    .is_some_and(|sources| sources.contains(&claim.source_revision));
                if !finished {
                    report.skip_release_store("publication_completion_unverified", 1);
                    continue;
                }
                // A signed pipeline run does not account for a separate tag
                // publisher at the same version.
                if inventory
                    .complete
                    .get(version)
                    .is_some_and(|families| families.contains(family_key(ReleaseFamily::Installer)))
                {
                    report.skip_release_store("installer_publication_untracked", 1);
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
                    remove_release_payloads(path)?;
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
                            complete_families = inventory
                                .complete
                                .get(version)
                                .map(|families| families.iter().copied().collect::<Vec<_>>().join(","))
                                .unwrap_or_else(|| "none".to_string()),
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
