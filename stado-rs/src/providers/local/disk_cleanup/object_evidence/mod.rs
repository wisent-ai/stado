//! Product run evidence in this host's object store.
//!
//! A product that writes run evidence into this host's object store owns how
//! long it is kept and can expire it with its own command. On a full host it
//! cannot: a host refusing jobs never claims the job carrying the product's
//! own retention command — the very work that would free the space. So the
//! host's own janitor takes it under the disk-full rule: every regular file
//! under [`ROOT`] except the fleet's pinned build inputs.

use std::collections::BTreeMap;
use std::path::Path;

use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "object_evidence";
/// Where product runs keep their evidence, relative to `$HOME`: the local
/// object store's `probierz/runs` prefix in the fleet namespace.
pub const ROOT: &str = ".stado/local-storage/ecosystem/probierz/runs";
/// The directory the fleet keeps its pinned, digest-addressed build inputs
/// in, wherever it appears. Nothing under it is run evidence.
const PINNED_INPUT_DIRECTORY: &str = "native-signing";
/// Stado's own release records directly under [`ROOT`]: every release run
/// (`release-pipeline`), build record (`build`) and change batch
/// (`release-changes`). The cleaner once took those too: at the disk-full
/// threshold on the host serving the fleet store it deleted every release run
/// and build mid-delivery, and `stado release status` answered `unknown
/// release product` for runs that had just published (f3e89522).
const RELEASE_RECORD_DIRECTORIES: &[&str] = &["release-pipeline", "build", "release-changes"];

/// Remove the run evidence under [`ROOT`].
pub fn scan_object_evidence(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    let root = home.join(ROOT);
    if !root.is_dir() {
        bump(&mut record.skipped, "root_absent");
        report.object_evidence = record;
        return;
    }
    let mut frontier = vec![root];
    while let Some(directory) = frontier.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            bump(&mut record.skipped, "directory_unreadable");
            continue;
        };
        for entry in entries.flatten() {
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
            // A pinned input is addressed by its own digest and is immutable.
            // Taking the fleet's Apple issuer chain and the pinned signer has
            // the next darwin release die in `macos-code-signing` with
            // `cannot read native signing input ... apple-issuers-<sha>.pem`;
            // taking a release record loses the run it records.
            let release_record = path
                .strip_prefix(home.join(ROOT))
                .ok()
                .and_then(|relative| relative.components().next())
                .is_some_and(|first| {
                    RELEASE_RECORD_DIRECTORIES
                        .iter()
                        .any(|kept| first.as_os_str() == *kept)
                });
            let kept = release_record
                || path
                    .components()
                    .any(|part| part.as_os_str() == PINNED_INPUT_DIRECTORY);
            if kept {
                bump(&mut record.skipped, "stado_record_or_pinned_input_kept");
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

fn bump(counts: &mut BTreeMap<String, i64>, reason: &str) {
    *counts.entry(reason.to_string()).or_default() += 1;
}
