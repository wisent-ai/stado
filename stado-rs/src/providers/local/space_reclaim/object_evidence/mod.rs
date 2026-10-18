//! Product run evidence in this host's object store.
//!
//! A product that writes run evidence into this host's object store owns how
//! long it is kept and can expire it with its own command. On a full host it
//! cannot: a host refusing jobs never claims the job carrying the product's
//! own retention command — the very work that would free the space. So the
//! host's own janitor takes it under the disk-full rule: run and result
//! evidence beneath the declared roots, but not pinned inputs or release records.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;

use super::janitor::state::report::{CleanerReport, CleanupReport};
use super::{ifmt, safefs, IFDIR, IFREG};
use crate::primitives::tally::Tally;

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "object_evidence";
/// Where Probierz keeps run evidence and dispatched results in the local
/// object store, relative to `$HOME`.
pub const ROOT: &str = ".stado/local-storage/ecosystem/probierz/runs";
pub const RESULTS_ROOT: &str = ".stado/local-storage/ecosystem/probierz/results";
/// The directory the fleet keeps its pinned, digest-addressed build inputs
/// in, wherever it appears. Nothing under it is run evidence.
const PINNED_INPUT_DIRECTORY: &str = "native-signing";
/// Stado's own release records directly under [`ROOT`]: every release run
/// (`release-pipeline`), build record (`build`) and change batch
/// (`release-changes`). These records are not disposable run evidence.
const RELEASE_RECORD_DIRECTORIES: &[&str] = &["release-pipeline", "build", "release-changes"];

/// Remove product evidence from both object-store prefixes. Each root is
/// opened from the selected home without following links, and every scan and
/// deletion is relative to a held directory descriptor.
pub fn scan_object_evidence(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    for (prefix, protect_release_records) in [(ROOT, true), (RESULTS_ROOT, false)] {
        scan_root(
            home,
            prefix,
            protect_release_records,
            enforcing,
            &mut record,
            report,
        );
    }
    report.object_evidence = record;
}

fn scan_root(
    home: &Path,
    prefix: &str,
    protect_release_records: bool,
    enforcing: bool,
    record: &mut CleanerReport,
    report: &mut CleanupReport,
) {
    let root = home.join(prefix);
    let Some(root_fd) = open_root(home, &root, prefix, record, report) else {
        return;
    };
    let mut frontier = vec![(root.clone(), root_fd)];
    while let Some((directory, directory_fd)) = frontier.pop() {
        let entries = match safefs::DirEntries::open(directory_fd.as_raw_fd()) {
            Ok(entries) => entries,
            Err(error) => {
                bump(&mut record.skipped, "directory_unreadable");
                refused(
                    report,
                    &format!("cannot read {}: {error}", directory.display()),
                );
                continue;
            }
        };
        for name in entries {
            let name = match name {
                Ok(name) => name,
                Err(error) => {
                    bump(&mut record.skipped, "unreadable");
                    refused(
                        report,
                        &format!("cannot read an entry of {}: {error}", directory.display()),
                    );
                    continue;
                }
            };
            record.scanned_items.tally();
            let path = directory.join(&name);
            // Stat relative to the held directory: no ancestor can turn into
            // a link between inspection and the eventual unlink.
            let info = match safefs::fstatat_nofollow(directory_fd.as_raw_fd(), &name) {
                Ok(info) => info,
                Err(error) => {
                    bump(&mut record.skipped, "unreadable");
                    refused(report, &format!("cannot read {}: {error}", path.display()));
                    continue;
                }
            };
            // Decide from the path beneath ROOT, not from the selected home's
            // name. A protected directory is opaque: its contents are neither
            // evidence nor something the cleaner needs permission to read.
            let relative = match path.strip_prefix(&root) {
                Ok(relative) => relative,
                Err(error) => {
                    bump(&mut record.skipped, "outside_root");
                    refused(
                        report,
                        &format!(
                            "{} is not under {}: {error}",
                            path.display(),
                            root.display()
                        ),
                    );
                    continue;
                }
            };
            let kept = protected(relative, protect_release_records);
            if ifmt(info.st_mode as u32) == IFDIR {
                if kept {
                    bump(&mut record.skipped, "stado_record_or_pinned_input_kept");
                    continue;
                }
                match safefs::open_dir_at(directory_fd.as_raw_fd(), &name) {
                    Ok(child) => frontier.push((path, child)),
                    Err(error) => {
                        bump(&mut record.skipped, "directory_unreadable");
                        refused(report, &format!("cannot open {}: {error}", path.display()));
                    }
                }
                continue;
            }
            if ifmt(info.st_mode as u32) != IFREG {
                bump(&mut record.skipped, "not_a_regular_file");
                continue;
            }
            // Release records and pinned inputs are needed for subsequent
            // delivery and are not disposable run evidence.
            if kept {
                bump(&mut record.skipped, "stado_record_or_pinned_input_kept");
                continue;
            }
            record.eligible_items.tally();
            record.expected_bytes += info.st_size;
            if !enforcing {
                continue;
            }
            match safefs::unlink_at(directory_fd.as_raw_fd(), &name) {
                Ok(()) => {
                    tally(&mut record.deleted_items);
                    record.actual_free_delta_bytes += info.st_size;
                }
                Err(error) => {
                    bump(&mut record.skipped, "deletion_refused");
                    refused(
                        report,
                        &format!("cannot remove {}: {error}", path.display()),
                    );
                }
            }
        }
    }
}

/// Open every component beneath the selected home without following a link.
/// Keeping the descriptors is the boundary; a path-only stat cannot protect
/// the subsequent scan when an entry is renamed between the two operations.
fn open_root(
    home: &Path,
    root: &Path,
    prefix: &str,
    record: &mut CleanerReport,
    report: &mut CleanupReport,
) -> Option<OwnedFd> {
    let mut directory = match safefs::open_dir_path(home) {
        Ok(directory) => directory,
        Err(error) => {
            bump(&mut record.skipped, "root_unreadable");
            refused(report, &format!("cannot open {}: {error}", home.display()));
            return None;
        }
    };
    let mut path = home.to_path_buf();
    for component in Path::new(prefix).components() {
        path.push(component);
        directory = match safefs::open_dir_at(directory.as_raw_fd(), component.as_os_str()) {
            Ok(child) => child,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                bump(&mut record.skipped, "root_absent");
                return None;
            }
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(code) if code == nix::libc::ENOTDIR || code == nix::libc::ELOOP
                ) =>
            {
                // A link or a file where the evidence root's path runs is
                // left alone, and said, so a root nothing is cleaned under
                // is told apart from one that does not exist.
                bump(&mut record.skipped, "root_not_a_directory");
                refused(
                    report,
                    &format!(
                        "{} under {} is not a directory this cleaner opens without following \
                         links: {error}",
                        path.display(),
                        root.display()
                    ),
                );
                return None;
            }
            Err(error) => {
                bump(&mut record.skipped, "root_unreadable");
                refused(
                    report,
                    &format!(
                        "cannot open {} under {}: {error}",
                        path.display(),
                        root.display()
                    ),
                );
                return None;
            }
        };
    }
    Some(directory)
}

/// The protected names are scoped to an object evidence root, not to HOME.
fn protected(relative: &Path, protect_release_records: bool) -> bool {
    let release_record = protect_release_records
        && relative.components().next().is_some_and(|first| {
            RELEASE_RECORD_DIRECTORIES
                .iter()
                .any(|kept| first.as_os_str() == *kept)
        });
    release_record
        || relative
            .components()
            .any(|part| part.as_os_str() == PINNED_INPUT_DIRECTORY)
}

/// Record why this cleaner could not read or remove a path.
fn refused(report: &mut CleanupReport, cause: &str) {
    report.add_error(CLEANER, &super::JanitorError::os(cause));
}

fn bump(counts: &mut BTreeMap<String, i64>, reason: &str) {
    counts.entry(reason.to_string()).or_default().tally();
}
