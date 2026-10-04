//! One pass of the release-store cleaner: the inventory walk, the decision
//! applied to every version it found, and the report it writes.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::providers::local::disk_cleanup::release_store::decision::retention_decision;
use crate::providers::local::disk_cleanup::release_store::inventory::families::complete_families;
use crate::providers::local::disk_cleanup::release_store::inventory::pins::{
    config_pinned_versions, host_pinned_versions,
};
use crate::providers::local::disk_cleanup::release_store::inventory::runs::run_retention_evidence;
use crate::providers::local::disk_cleanup::release_store::inventory::tree_bytes;
use crate::providers::local::disk_cleanup::release_store::{
    family_key, ProductReleases, ReleaseFamily, CLEANER, RELEASES_ROOT, STATE_DIR,
};
use crate::providers::local::disk_cleanup::{euid, free_bytes, CleanupReport, JanitorError};

use super::{remove_release_payloads, source_revision};

/// Reclaim release versions nothing on this host or in the fleet still uses.
///
/// `declared_pins` is [`declared_versions`](super::super::declared_versions)
/// over the canonical registry. It is passed in rather than fetched here
/// because this function must not perform network I/O; `None` means the
/// registry did not answer, and then this cleaner removes nothing, because
/// it cannot know which versions other hosts run.
pub fn scan_release_store(
    home: &Path,
    enforcing: bool,
    declared_pins: Option<&BTreeMap<String, BTreeSet<String>>>,
    report: &mut CleanupReport,
) {
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let Some(declared_pins) = declared_pins else {
            report.skip_release_store("registry_unreadable", 1);
            return Ok(());
        };
        let releases = home.join(RELEASES_ROOT);
        if !releases.is_dir() {
            report.skip_release_store("root_absent", 1);
            return Ok(());
        }
        let state_dir = home.join(STATE_DIR);
        let ecosystem = releases
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".stado/local-storage/ecosystem"));
        let host_pins = host_pinned_versions(&state_dir);
        let config_pins = config_pinned_versions(home);
        let run_evidence = run_retention_evidence(&ecosystem)?;
        let run_pins = &run_evidence.pinned;
        let home_device = std::fs::metadata(home)?.dev();

        // Inventory: every product directory, every version directory under it.
        let mut products: BTreeMap<String, ProductReleases> = BTreeMap::new();
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
                if !enforcing {
                    continue;
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
                        // reclaim: when a complete installable version is
                        // removed and the only trace is a counter reading
                        // `deleted_items`, the loss gets reconstructed from
                        // a 404 later rather than read from the log. It names
                        // what was removed
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
                            "reclaimed a release version under the disk-full rule"
                        );
                        report.release_store.actual_free_delta_bytes += delta.max(0);
                        report.release_store.deleted_items += 1;
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
