//! Logs the coding-agent harnesses write under this account's home.
//!
//! A harness keeps its own logs and never removes them: OMP writes one
//! request dump of several megabytes for every request a provider refuses,
//! and a host running agents all day holds tens of gigabytes of them that no
//! product reads again. The roots are fixed here and are each harness's log
//! directory only; under the disk-full rule every file in them goes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "agent_logs";

/// Home-relative log directories of the harnesses Jeden and Tama drive.
pub const LOG_ROOTS: [&str; 5] = [
    ".omp/logs",
    ".claude/debug",
    ".codex/log",
    ".factory/logs",
    ".kimi-code/logs",
];

/// Remove every log file under every harness log root.
pub fn scan_agent_logs(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    let mut frontier: Vec<PathBuf> = LOG_ROOTS
        .iter()
        .map(|root| home.join(root))
        .filter(|root| root.is_dir())
        .collect();
    while let Some(directory) = frontier.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            bump(&mut record.skipped, "directory_unreadable");
            continue;
        };
        for entry in entries.flatten() {
            record.scanned_items += 1;
            let path = entry.path();
            // `entry.metadata` does not follow a symbolic link, so a link
            // out of a log directory is never walked or removed.
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
    report.agent_logs = record;
}

fn bump(counts: &mut BTreeMap<String, i64>, reason: &str) {
    *counts.entry(reason.to_string()).or_default() += 1;
}
