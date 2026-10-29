//! Product run evidence in this host's object store.
//!
//! A product that writes run evidence into this host's object store owns how
//! long it is kept and can expire it with its own command. On a full host it
//! cannot: a host refusing jobs never claims the job carrying the product's
//! own retention command — the very work that would free the space. So the
//! host's own janitor takes it under the disk-full rule: run and result
//! evidence beneath the declared roots, but not pinned inputs, release
//! records or the queue's own run records.

use std::io::ErrorKind;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;

use super::janitor::state::report::{met::Met, CleanupReport};
use super::{ifmt, safefs, IFDIR, IFREG};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "object_evidence";
/// Where Probierz keeps run evidence and dispatched results in the local
/// object store, relative to `$HOME`.
pub const ROOT: &str = ".stado/local-storage/ecosystem/probierz/runs";
pub const RESULTS_ROOT: &str = ".stado/local-storage/ecosystem/probierz/results";
/// The directory the fleet keeps its pinned, digest-addressed build inputs
/// in, wherever it appears. Nothing under it is run evidence.
const PINNED_INPUT_DIRECTORY: &str = "native-signing";
/// Stado's own records in directories directly under [`ROOT`]: every release
/// run (`release-pipeline`, `release-pipeline-unfinished`), build record
/// (`build`), change batch (`release-changes`), the source bundle a placed
/// quality check waits on (`quality`) and the context a queued workload
/// session is started with (`workload-session`). These records are not
/// disposable run evidence.
const RELEASE_RECORD_DIRECTORIES: &[&str] = &[
    "release-pipeline",
    "release-pipeline-unfinished",
    "build",
    "release-changes",
    "quality",
    "workload-session",
];

/// Remove product evidence from both object-store prefixes. Each root is
/// opened from the selected home without following links, and every scan and
/// deletion is relative to a held directory descriptor.
pub fn scan_object_evidence(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut met = Met::default();
    for (prefix, protect_release_records) in [(ROOT, true), (RESULTS_ROOT, false)] {
        scan_root(
            home,
            prefix,
            protect_release_records,
            enforcing,
            &mut met,
            report,
        );
    }
    match met.report() {
        Ok(record) => report.object_evidence = record,
        Err(cause) => refused(report, &cause),
    }
}

fn scan_root(
    home: &Path,
    prefix: &str,
    protect_release_records: bool,
    enforcing: bool,
    met: &mut Met,
    report: &mut CleanupReport,
) {
    let root = home.join(prefix);
    let Some(root_fd) = open_root(home, &root, prefix, met, report) else {
        return;
    };
    let mut frontier = vec![(root.clone(), root_fd)];
    while let Some((directory, directory_fd)) = frontier.pop() {
        let entries = match safefs::DirEntries::open(directory_fd.as_raw_fd()) {
            Ok(entries) => entries,
            Err(error) => {
                met.skipped("directory_unreadable");
                refused(
                    report,
                    &format!("cannot read {}: {error}", directory.display()),
                );
                continue;
            }
        };
        let names: Vec<_> = entries.collect();
        met.listed(names.len());
        for name in names {
            let name = match name {
                Ok(name) => name,
                Err(error) => {
                    met.skipped("unreadable");
                    refused(
                        report,
                        &format!("cannot read an entry of {}: {error}", directory.display()),
                    );
                    continue;
                }
            };
            let path = directory.join(&name);
            // Stat relative to the held directory: no ancestor can turn into
            // a link between inspection and the eventual unlink.
            let info = match safefs::fstatat_nofollow(directory_fd.as_raw_fd(), &name) {
                Ok(info) => info,
                Err(error) => {
                    met.skipped("unreadable");
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
                    met.skipped("outside_root");
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
            // A regular file directly under ROOT is the queue's own record of
            // a run (`runs/<run_id>.json`, the retained-jobs index): products
            // keep their evidence in directories beneath it. Deleting one
            // left every submission refused with "run manifest … disappeared"
            // and every finished run without the record its reaper retains
            // outcomes into.
            let queue_record = protect_release_records
                && ifmt(info.st_mode as u32) == IFREG
                && relative.components().count() == 1;
            let kept = queue_record || protected(relative, protect_release_records);
            if ifmt(info.st_mode as u32) == IFDIR {
                if kept {
                    met.skipped("stado_record_or_pinned_input_kept");
                    continue;
                }
                match safefs::open_dir_at(directory_fd.as_raw_fd(), &name) {
                    Ok(child) => frontier.push((path, child)),
                    Err(error) => {
                        met.skipped("directory_unreadable");
                        refused(report, &format!("cannot open {}: {error}", path.display()));
                    }
                }
                continue;
            }
            if ifmt(info.st_mode as u32) != IFREG {
                met.skipped("not_a_regular_file");
                continue;
            }
            // Release records and pinned inputs are needed for subsequent
            // delivery and are not disposable run evidence.
            if kept {
                met.skipped("stado_record_or_pinned_input_kept");
                continue;
            }
            met.eligible(info.st_size.unsigned_abs());
            if !enforcing {
                continue;
            }
            match safefs::unlink_at(directory_fd.as_raw_fd(), &name) {
                Ok(()) => met.removed(Some(info.st_size.unsigned_abs())),
                Err(error) => {
                    met.skipped("deletion_refused");
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
    met: &mut Met,
    report: &mut CleanupReport,
) -> Option<OwnedFd> {
    let mut directory = match safefs::open_dir_path(home) {
        Ok(directory) => directory,
        Err(error) => {
            met.skipped("root_unreadable");
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
                met.skipped("root_absent");
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
                met.skipped("root_not_a_directory");
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
                met.skipped("root_unreadable");
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
