//! The compiler cache's store.
//!
//! Every Cargo build Stado runs compiles through Kache, which keeps one store
//! per account (`~/Library/Caches/kache` on macOS, `~/.cache/kache` on
//! Linux) and never shrinks it. No cleaner covered it: `stado space report`
//! read it uncovered at 19.2 GiB on the always-on Mac and 5.2 GiB on the Linux
//! builder while both sat at the disk-full threshold. Under the disk-full rule
//! the store goes like any cache — what it costs is a slower next build — but
//! not while a job runs, because a running build is reading and writing it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "compiler_cache";

/// Home-relative stores Kache keeps, by platform.
pub const STORE_ROOTS: &[&str] = &["Library/Caches/kache", ".cache/kache"];

/// Remove every file of the compiler cache's store, unless a job is running.
pub fn scan_compiler_cache(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    let mut skipped: Vec<&'static str> = Vec::new();
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    let mut frontier: Vec<PathBuf> = STORE_ROOTS
        .iter()
        .map(|root| home.join(root))
        .filter(|root| root.is_dir())
        .collect();
    while let Some(directory) = frontier.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            skipped.push("directory_unreadable");
            continue;
        };
        for entry in entries.flatten() {
            // `symlink_metadata` does not follow a link, so a link out of the
            // store is never walked or removed.
            let Ok(info) = std::fs::symlink_metadata(entry.path()) else {
                skipped.push("unreadable");
                continue;
            };
            if info.is_dir() {
                frontier.push(entry.path());
            } else if info.is_file() {
                files.push((entry.path(), info.len()));
            } else {
                skipped.push("not_a_regular_file");
            }
        }
    }
    record.scanned_items = files.len() as i64;
    record.eligible_items = files.len() as i64;
    record.expected_bytes = files.iter().map(|(_, bytes)| *bytes).sum::<u64>() as i64;
    if report.active_job_count > 0 {
        skipped.extend(files.iter().map(|_| "job_running"));
        files.clear();
    }
    if enforcing {
        let mut freed: Vec<u64> = Vec::new();
        for (path, bytes) in files {
            match std::fs::remove_file(&path) {
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
        record.deleted_items = freed.len() as i64;
        record.actual_free_delta_bytes = freed.iter().sum::<u64>() as i64;
    }
    let reasons: BTreeSet<&str> = skipped.iter().copied().collect();
    record.skipped = reasons
        .into_iter()
        .map(|reason| {
            let count = skipped.iter().filter(|seen| **seen == reason).count() as i64;
            (reason.to_string(), count)
        })
        .collect::<BTreeMap<String, i64>>();
    report.compiler_cache = record;
}
