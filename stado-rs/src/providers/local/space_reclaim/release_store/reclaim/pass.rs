//! One pass of the release-store cleaner: the inventory walk, the decision
//! applied to every version it found, and the report it writes.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::providers::local::space_reclaim::janitor::state::report::met::Met;

use crate::providers::local::space_reclaim::release_store::decision::retention_decision;
use crate::providers::local::space_reclaim::release_store::inventory::pins::{
    config_pinned_versions, host_pinned_versions,
};
use crate::providers::local::space_reclaim::release_store::inventory::runs::run_retention_evidence;
use crate::providers::local::space_reclaim::release_store::inventory::signed::signed_release_complete;
use crate::providers::local::space_reclaim::release_store::inventory::tree_bytes;
use crate::providers::local::space_reclaim::release_store::{
    ProductReleases, CLEANER, RELEASES_ROOT, STATE_DIR,
};
use crate::providers::local::space_reclaim::{euid, free_bytes, CleanupReport, JanitorError};

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
    let mut met = Met::default();
    let body = |met: &mut Met, report: &mut CleanupReport| -> Result<(), JanitorError> {
        let Some(declared_pins) = declared_pins else {
            met.skipped("registry_unreadable");
            return Ok(());
        };
        let releases = home.join(RELEASES_ROOT);
        if !releases.is_dir() {
            met.skipped("root_absent");
            return Ok(());
        }
        let state_dir = home.join(STATE_DIR);
        let Some(ecosystem) = releases.parent() else {
            return Err(JanitorError::os(&format!(
                "the release store {} has no namespace directory above it",
                releases.display()
            )));
        };
        let ecosystem = ecosystem.to_path_buf();
        let host_pins = host_pinned_versions(&state_dir)?;
        let config_pins = config_pinned_versions(home)?;
        let run_evidence = run_retention_evidence(&ecosystem)?;
        let run_pins = &run_evidence.pinned;
        let home_device = std::fs::metadata(home)?.dev();

        // Inventory: every product directory, every version directory under it.
        let mut products: BTreeMap<String, ProductReleases> = BTreeMap::new();
        for product_entry in std::fs::read_dir(&releases)? {
            // An entry the store cannot list is a skipped item with its
            // reason, never left out of the inventory without a trace.
            let Ok(product_entry) = product_entry else {
                met.skipped("product_entry_unreadable");
                continue;
            };
            let product_path = product_entry.path();
            let Ok(product_info) = std::fs::symlink_metadata(&product_path) else {
                met.skipped("product_stat_failed");
                continue;
            };
            if !product_info.is_dir()
                || product_info.uid() != euid()
                || product_info.dev() != home_device
            {
                met.skipped("product_not_an_owned_directory");
                continue;
            }
            let product = product_entry.file_name().to_string_lossy().to_string();
            let Ok(versions) = std::fs::read_dir(&product_path) else {
                met.skipped("read_dir_failed");
                continue;
            };
            let versions: Vec<_> = versions.collect();
            met.listed(versions.len());
            for version_entry in versions {
                let Ok(version_entry) = version_entry else {
                    met.skipped("entry_unreadable");
                    continue;
                };
                let version_path = version_entry.path();
                let Ok(info) = std::fs::symlink_metadata(&version_path) else {
                    met.skipped("stat_failed");
                    continue;
                };
                if !info.is_dir() || info.uid() != euid() || info.dev() != home_device {
                    met.skipped("not_an_owned_directory");
                    continue;
                }
                let version = version_entry.file_name().to_string_lossy().to_string();
                // A version this pass cannot list is neither judged nor
                // reclaimed: its completeness and size are unknown.
                let complete = match signed_release_complete(&version_path) {
                    Ok(complete) => complete,
                    Err(error) => {
                        let detail = format!(
                            "release version {} cannot be listed, so it is left out of reclaim: {error}",
                            version_path.display()
                        );
                        report.add_error(CLEANER, &JanitorError::os(&detail));
                        continue;
                    }
                };
                let bytes = tree_bytes(&version_path);
                let inventory = products.entry(product.clone()).or_default();
                if complete {
                    inventory.complete.insert(version.clone());
                }
                inventory.versions.insert(version, (version_path, bytes));
            }
        }

        for (product, inventory) in &products {
            let served_here = host_pins.contains_key(product) || run_pins.contains_key(product);
            if !served_here {
                // Each version of a product this host serves no release of
                // is one skip.
                for _ in inventory.versions.keys() {
                    met.skipped("product_not_served_here");
                }
                continue;
            }
            let none = BTreeSet::new();
            let versions: Vec<String> = inventory.versions.keys().cloned().collect();
            let decisions = retention_decision(
                &versions,
                &inventory.complete,
                pinned(&host_pins, product, &none),
                pinned(declared_pins, product, &none),
                pinned(&config_pins, product, &none),
                pinned(run_pins, product, &none),
            );
            for (version, keep) in decisions {
                let (path, bytes) = &inventory.versions[version];
                if let Some(reason) = keep {
                    met.skipped(reason);
                    continue;
                }
                let Some(claim) = source_revision(path, product, version) else {
                    met.skipped("source_identity_unverified");
                    continue;
                };
                let finished = run_evidence
                    .finished
                    .get(product)
                    .and_then(|versions| versions.get(version))
                    .is_some_and(|sources| sources.contains(&claim.source_revision));
                if !finished {
                    met.skipped("publication_completion_unverified");
                    continue;
                }
                met.eligible(bytes.unsigned_abs());
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
                            signed_release_complete = inventory.complete.contains(version),
                            not_pinned_by =
                                "host_state, host_declaration, config_release_version, pipeline_run",
                            "reclaimed a release version under the disk-full rule"
                        );
                        // Free space another writer took meanwhile is no
                        // space this removal freed: only a gain is counted.
                        met.removed(delta.is_positive().then(|| delta.unsigned_abs()));
                    }
                    Err(exc) => report.add_error(CLEANER, &exc),
                }
            }
        }
        Ok(())
    };
    if let Err(exc) = body(&mut met, report) {
        report.add_error(CLEANER, &exc);
    }
    match met.report() {
        Ok(record) => report.release_store = record,
        Err(cause) => report.add_error(CLEANER, &JanitorError::os(&cause)),
    }
}

/// The versions `pins` holds for `product`; a product it names nowhere has
/// none pinned there.
fn pinned<'a>(
    pins: &'a BTreeMap<String, BTreeSet<String>>,
    product: &str,
    none: &'a BTreeSet<String>,
) -> &'a BTreeSet<String> {
    match pins.get(product) {
        Some(versions) => versions,
        None => none,
    }
}
