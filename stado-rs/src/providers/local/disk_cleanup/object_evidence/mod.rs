//! Aged product evidence under a declared object-store root.
//!
//! A product that writes run evidence into this host's object store owns how
//! long it is kept and can expire it with its own command. On a host under
//! disk pressure it cannot: `charless-mac-mini` published `not accepting
//! jobs: disk_pressure_active` at 3.7 GiB free, so the job carrying
//! `probierz retention --fleet --apply` — the very work that would have
//! freed the 34.9 GiB under `ecosystem/probierz` — sat in the queue and
//! could never be claimed. Reclamation that depends on job admission cannot
//! reach the host that needs it most.
//!
//! So the host's own janitor takes it, under the rule every other cleaner
//! follows: only inside a root the operator declared, only files older than
//! the declared age, bounded by the pass's own scan and time budgets, and
//! never in a planning pass. The declaration is what makes it safe — nothing
//! is swept because it happens to sit in the store.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::janitor::state::report::{CleanerReport, CleanupReport};
use crate::targets::DiskCleanupPolicy;

/// The key under `targets[].disk_cleanup.cleaners`.
pub const CLEANER: &str = "object_evidence";
/// The directory the fleet keeps its pinned, digest-addressed build inputs
/// in. Nothing under it is run evidence, and nothing under it expires.
const PINNED_INPUT_DIRECTORY: &str = "native-signing";

/// Expire the evidence under this host's declared object-evidence root.
pub fn scan_object_evidence(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    enforcing: bool,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get(CLEANER) else {
        return;
    };
    let mut record = CleanerReport::default();
    let Some(declared) = configured.root.as_deref().filter(|value| !value.is_empty()) else {
        // A declaration with no root sweeps nothing: which namespace and
        // prefix are meant is exactly what the root says.
        bump(&mut record.skipped, "root_undeclared");
        report.object_evidence = record;
        return;
    };
    let root = if Path::new(declared).is_absolute() {
        PathBuf::from(declared)
    } else {
        home.join(declared)
    };
    if !root.is_dir() {
        bump(&mut record.skipped, "root_absent");
        report.object_evidence = record;
        return;
    }
    let min_age = configured.min_age_seconds.max(0) as f64;
    let mut budget = remaining_scan;
    let mut frontier = vec![root];
    while let Some(directory) = frontier.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            bump(&mut record.skipped, "directory_unreadable");
            continue;
        };
        for entry in entries.flatten() {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                bump(&mut record.skipped, "scan_deadline");
                report.object_evidence = record;
                return;
            }
            if budget <= 0 {
                report.caps.scan = true;
                bump(&mut record.skipped, "scan_cap");
                report.object_evidence = record;
                return;
            }
            budget -= 1;
            record.scanned_items += 1;
            let path = entry.path();
            let Ok(info) = entry.metadata() else {
                bump(&mut record.skipped, "unreadable");
                continue;
            };
            if info.is_dir() {
                frontier.push(path);
                continue;
            }
            if !info.is_file() {
                bump(&mut record.skipped, "not_a_regular_file");
                continue;
            }
            // A pinned input is addressed by its own digest and is immutable,
            // so its age says nothing about whether anything still needs it.
            // On 2026-09-21 a pass over `ecosystem/probierz/artifacts` took
            // the fleet's Apple issuer chain and the pinned signer with it,
            // and the next darwin release died in `macos-code-signing` with
            // `cannot read native signing input ... apple-issuers-<sha>.pem`.
            if path
                .components()
                .any(|part| part.as_os_str() == PINNED_INPUT_DIRECTORY)
            {
                bump(&mut record.skipped, "pinned_input_kept");
                continue;
            }
            if now - modified_seconds(&info) < min_age {
                bump(&mut record.skipped, "younger_than_min_age");
                continue;
            }
            record.eligible_items += 1;
            record.expected_bytes += info.len() as i64;
            if !enforcing {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    record.deleted_items += 1;
                    record.actual_free_delta_bytes += info.len() as i64;
                }
                Err(error) => {
                    bump(&mut record.skipped, "deletion_refused");
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
    report.object_evidence = record;
}

fn modified_seconds(info: &std::fs::Metadata) -> f64 {
    use std::os::unix::fs::MetadataExt;
    info.mtime() as f64
}

fn bump(counts: &mut BTreeMap<String, i64>, reason: &str) {
    *counts.entry(reason.to_string()).or_default() += 1;
}
