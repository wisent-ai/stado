//! Logs the coding-agent harnesses write under this account's home.
//!
//! A harness keeps its own logs and never removes them: OMP writes one
//! request dump of several megabytes for every request a provider refuses,
//! and a host running agents all day holds tens of gigabytes of them that no
//! product reads again. The roots are fixed here and are each harness's log
//! directory only; under the disk-full rule every file in them goes.

use std::path::{Path, PathBuf};

use super::janitor::state::report::{met::Met, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "agent_logs";

/// Home-relative log directories of the harnesses Jeden and Tama drive.
pub const LOG_ROOTS: &[&str] = &[
    ".omp/logs",
    ".claude/debug",
    ".codex/log",
    ".factory/logs",
    ".kimi-code/logs",
];

/// Remove every log file under every harness log root.
pub fn scan_agent_logs(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut frontier: Vec<PathBuf> = LOG_ROOTS
        .iter()
        .map(|root| home.join(root))
        .filter(|root| root.is_dir())
        .collect();
    let mut met = Met::default();
    while let Some(directory) = frontier.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            met.skipped("directory_unreadable");
            continue;
        };
        let entries: Vec<_> = entries.collect();
        met.listed(entries.len());
        for entry in entries {
            // An entry the directory cannot list is counted as skipped,
            // never left out of the scan without a trace.
            let Ok(entry) = entry else {
                met.skipped("entry_unreadable");
                continue;
            };
            let path = entry.path();
            // `entry.metadata` does not follow a symbolic link, so a link
            // out of a log directory is never walked or removed.
            let Ok(info) = entry.metadata() else {
                met.skipped("unreadable");
                continue;
            };
            if info.is_dir() {
                frontier.push(path);
                continue;
            }
            if !info.is_file() {
                met.skipped("not_a_regular_file");
                continue;
            }
            met.eligible(info.len());
            if !enforcing {
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => met.removed(Some(info.len())),
                Err(error) => {
                    met.skipped("deletion_refused");
                    report.add_error(
                        CLEANER,
                        &super::JanitorError::os(&format!("cannot remove {}: {error}", path.display())),
                    );
                }
            }
        }
    }
    match met.report() {
        Ok(record) => report.agent_logs = record,
        Err(cause) => report.add_error(CLEANER, &super::JanitorError::os(&cause)),
    }
}
