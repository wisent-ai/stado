//! Terminal queue-job workdir cleanup.
//!
//! The local agent owns every job tree under
//! `$HOME/.stado/work/jobs/wc-<job_id>`. This owner-visible persistent root is
//! deliberate: a workload's cwd and open output descriptor survive an unlink,
//! but their pathname does not. When job trees lived in `/tmp`, external temp
//! cleanup could therefore remove a running job's diagnostics while the
//! janitor's workload lock was held. Release jobs submitted through an older
//! agent leave a narrow compatibility symlink at `/tmp/wc-<job_id>` so that the
//! old agent can finish heartbeats and artifact upload after relocating the
//! tree; this cleaner removes only such owner-matched symlinks after their jobs
//! are terminal.
//!
//! A job workdir is safe to remove when its job is neither queued nor running.
//! Age is not the gate: build workdirs can fill a host within minutes, while a
//! live job must be retained indefinitely. An unreadable queue store therefore
//! removes nothing rather than guessing. Tree deletion is dir-fd-relative,
//! non-following, same-device and holds the canonical root throughout. The only
//! operation under `/tmp` is unlinking a direct-child symlink whose exact
//! canonical target and terminal job id have both been verified.
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use nix::libc::dev_t;
use nix::sys::stat::{FileStat, Mode};

use super::safefs;
use super::weles::dir_size;
use super::{euid, free_bytes, ifmt, CleanupReport, JanitorError, GIB, IFDIR};
use crate::targets::DiskCleanupPolicy;

/// The cleaner's registry name, and the key its counts appear under in the
/// janitor's report. Declared here rather than spelled at each use, because
/// [`crate::targets`]'s allowed-cleaner list, the report, and this scan have to
/// name the same cleaner or a policy authorizes a pass that never runs.
pub const CLEANER: &str = "queue_workdirs";

/// Directory name prefix for queue-owned job workdirs.
pub const WORKDIR_PREFIX: &str = "wc-";

/// Compatibility root used only by agents that predate the persistent root.
const LEGACY_WORK_ROOT: &str = "/tmp";
/// Queue-owned workdir root, relative to the account that runs the agent.
pub const WORK_ROOT: &str = ".stado/work/jobs";

/// Queue root components below the already-resolved agent home. Each is opened
/// separately with `O_DIRECTORY|O_NOFOLLOW`; no component symlink is supported.
const WORK_ROOT_COMPONENTS: [&str; 3] = [".stado", "work", "jobs"];

/// Canonical owner-visible root beneath an already-resolved agent home.
pub fn work_root_in(home: &Path) -> PathBuf {
    home.join(WORK_ROOT)
}

fn resolved_home(home: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(home)
}

/// Canonical owner-visible root for every local queue job.
pub fn work_root() -> PathBuf {
    let home = crate::config_file::expand_tilde("~");
    work_root_in(&resolved_home(&home).unwrap_or(home))
}

fn validate_owned_directory(fd: RawFd, label: &Path) -> io::Result<FileStat> {
    let info = safefs::fstat(fd)?;
    if ifmt(info.st_mode as u32) != IFDIR || info.st_uid != euid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("unsafe queue directory {}", label.display()),
        ));
    }
    Ok(info)
}

fn open_owned_component(
    parent: RawFd,
    name: &OsStr,
    label: &Path,
    create: bool,
) -> io::Result<OwnedFd> {
    let opened = match safefs::open_dir_at(parent, name) {
        Ok(fd) => fd,
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
            match safefs::mkdir_at(parent, name, Mode::from_bits_truncate(0o700)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            safefs::open_dir_at(parent, name)?
        }
        Err(error) => return Err(error),
    };
    validate_owned_directory(opened.as_raw_fd(), label)?;
    safefs::fchmod(opened.as_raw_fd(), Mode::from_bits_truncate(0o700))?;
    Ok(opened)
}

fn open_work_root_in(home: &Path, create: bool) -> io::Result<(PathBuf, OwnedFd, dev_t)> {
    let home = resolved_home(home)?;
    let home_fd = safefs::open_dir_path(&home)?;
    let home_info = validate_owned_directory(home_fd.as_raw_fd(), &home)?;
    let mut parent = home_fd;
    let mut path = home.clone();
    for component in WORK_ROOT_COMPONENTS {
        path.push(component);
        parent = open_owned_component(parent.as_raw_fd(), OsStr::new(component), &path, create)?;
    }
    Ok((path, parent, home_info.st_dev))
}

/// Canonical workdir for one validated local queue job.
pub fn work_dir(job_id: &str) -> io::Result<PathBuf> {
    if !valid_job_id(job_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "queue job id must be job- followed by 24 lowercase hex digits",
        ));
    }
    Ok(work_root().join(format!("{WORKDIR_PREFIX}{job_id}")))
}

fn valid_job_id(id: &str) -> bool {
    crate::queue::submit::is_canonical_job_id(id)
}

/// Create one canonical job tree without accepting a symlink or foreign owner.
pub fn create_work_dir(job_id: &str) -> io::Result<PathBuf> {
    if !valid_job_id(job_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "queue job id must be job- followed by 24 lowercase hex digits",
        ));
    }
    let (root, root_fd, _) = open_work_root_in(&crate::config_file::expand_tilde("~"), true)?;
    let name = OsString::from(format!("{WORKDIR_PREFIX}{job_id}"));
    let work = root.join(&name);
    let work_fd = open_owned_component(root_fd.as_raw_fd(), &name, &work, true)?;
    let output = work.join("output");
    open_owned_component(work_fd.as_raw_fd(), OsStr::new("output"), &output, true)?;
    Ok(work)
}

/// The safe job id encoded by a canonical workdir name.
fn job_id(name: &str) -> Option<&str> {
    let id = name.strip_prefix(WORKDIR_PREFIX)?;
    valid_job_id(id).then_some(id)
}

const MAX_WORKDIR_DEPTH: usize = 256;

fn same_object(first: &FileStat, second: &FileStat) -> bool {
    first.st_dev == second.st_dev
        && first.st_ino == second.st_ino
        && first.st_mode == second.st_mode
        && first.st_uid == second.st_uid
}

fn entry_names(dir_fd: RawFd) -> Result<BTreeSet<OsString>, JanitorError> {
    let mut names = BTreeSet::new();
    for name in safefs::DirEntries::open(dir_fd)? {
        let name = name?;
        if name != "." && name != ".." {
            names.insert(name);
        }
    }
    Ok(names)
}

fn remove_contents_at(dir_fd: RawFd, root_dev: dev_t, depth: usize) -> Result<(), JanitorError> {
    for name in entry_names(dir_fd)? {
        let info = safefs::fstatat_nofollow(dir_fd, &name)?;
        if ifmt(info.st_mode as u32) != IFDIR {
            safefs::unlink_at(dir_fd, &name)?;
            continue;
        }
        if info.st_dev != root_dev {
            return Err(JanitorError::os("queue workdir spans a device boundary"));
        }
        if depth + 1 > MAX_WORKDIR_DEPTH {
            return Err(JanitorError::os("queue workdir nested too deeply"));
        }
        let child = safefs::open_dir_at(dir_fd, &name)?;
        if !same_object(&safefs::fstat(child.as_raw_fd())?, &info) {
            return Err(JanitorError::os(
                "queue workdir entry replaced while deleting",
            ));
        }
        remove_contents_at(child.as_raw_fd(), root_dev, depth + 1)?;
        drop(child);
        safefs::rmdir_at(dir_fd, &name)?;
    }
    Ok(())
}

fn remove_tree_at(
    root_fd: RawFd,
    name: &OsStr,
    work_fd: RawFd,
    root_dev: dev_t,
) -> Result<(), JanitorError> {
    remove_contents_at(work_fd, root_dev, 0)?;
    safefs::rmdir_at(root_fd, name)?;
    Ok(())
}

/// Evict the workdirs of terminal jobs, oldest scan order first.
///
/// `live_jobs` is the keep-list: every job id currently in `queue` or `running`.
/// `None` means the queue store could not be read this pass, and this cleaner
/// then removes nothing at all.
pub fn scan_queue_workdirs(
    home: &Path,
    policy: &DiskCleanupPolicy,
    _now: f64,
    remaining_scan: i64,
    deadline: Instant,
    live_jobs: Option<&[String]>,
    report: &mut CleanupReport,
) {
    let Some(_configured) = policy.cleaners.get(CLEANER) else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        // Without the keep-list there is no terminal-job gate, so this cleaner
        // removes nothing rather than risk a live job's persistent tree.
        let Some(live_jobs) = live_jobs else {
            report.skip_workdirs("queue_store_unreadable", 1);
            return Ok(());
        };
        // Admission and cleanup traverse the same three owned components from
        // the physically resolved home. Every component is O_DIRECTORY |
        // O_NOFOLLOW, so replacing `.stado`, `work`, or `jobs` with a symlink
        // cannot redirect this pass.
        let (canonical_root, root_fd, home_device) = match open_work_root_in(home, false) {
            Ok(opened) => opened,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                report.skip_workdirs("root_absent", 1);
                return Ok(());
            }
            Err(_) => {
                report.skip_workdirs("unsafe_root", 1);
                return Ok(());
            }
        };
        let root_info = safefs::fstat(root_fd.as_raw_fd())?;
        if root_info.st_dev != home_device {
            report.skip_workdirs("unsafe_root", 1);
            return Ok(());
        }
        // Reserve at most one eighth (and never more than 16 entries) for old
        // agent links. Canonical work stays dominant while a full canonical
        // root cannot consume the legacy pass's whole share.
        let legacy_budget = (remaining_scan / 8)
            .clamp(1, 16)
            .min(remaining_scan.saturating_sub(1));
        let mut ordered: Vec<OsString> = Vec::new();
        let mut budget = remaining_scan - legacy_budget;
        for name in safefs::DirEntries::open(root_fd.as_raw_fd())? {
            let name = name?;
            if !name.to_string_lossy().starts_with(WORKDIR_PREFIX) {
                continue;
            }
            ordered.push(name);
            budget -= 1;
            if budget <= 0 {
                report.caps.scan = true;
                report.skip_workdirs("scan_cap", 1);
                break;
            }
        }
        ordered.sort();
        let mut deleted_bytes = 0i64;
        for name in ordered {
            if Instant::now() >= deadline {
                report.caps.deadline = true;
                report.skip_workdirs("scan_deadline", 1);
                break;
            }
            report.workdirs.scanned_items += 1;
            let info = match safefs::fstatat_nofollow(root_fd.as_raw_fd(), &name) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_workdirs("stat_failed", 1);
                    continue;
                }
            };
            if ifmt(info.st_mode as u32) != IFDIR {
                report.skip_workdirs("not_workdir", 1);
                continue;
            }
            if info.st_uid != euid() || info.st_dev != root_info.st_dev {
                report.skip_workdirs("unsafe_owner_or_device", 1);
                continue;
            }
            let work_fd = match safefs::open_dir_at(root_fd.as_raw_fd(), &name) {
                Ok(work_fd) => work_fd,
                Err(_) => {
                    report.skip_workdirs("entry_replaced", 1);
                    continue;
                }
            };
            if !same_object(&safefs::fstat(work_fd.as_raw_fd())?, &info) {
                report.skip_workdirs("entry_replaced", 1);
                continue;
            }
            let name_text = name.to_string_lossy();
            let Some(id) = job_id(&name_text) else {
                report.skip_workdirs("not_workdir", 1);
                continue;
            };
            if live_jobs.iter().any(|live| live == id) {
                report.skip_workdirs("job_queued_or_running", 1);
                continue;
            }
            report.workdirs.eligible_items += 1;
            let expected = dir_size(&canonical_root.join(&name));
            report.workdirs.expected_bytes += expected;
            if policy.mode != "enforce" {
                continue;
            }
            if report.workdirs.deleted_items >= policy.max_items_per_pass {
                report.caps.items = true;
                report.skip_workdirs("item_cap", 1);
                continue;
            }
            if deleted_bytes >= policy.max_bytes_per_pass {
                report.caps.bytes = true;
                report.skip_workdirs("byte_cap", 1);
                continue;
            }
            if free_bytes(home)? >= policy.target_free_gb * GIB {
                break;
            }
            let delete_attempt = (|| -> Result<i64, JanitorError> {
                let before = free_bytes(home)?;
                remove_tree_at(
                    root_fd.as_raw_fd(),
                    &name,
                    work_fd.as_raw_fd(),
                    root_info.st_dev,
                )?;
                Ok(free_bytes(home)? - before)
            })();
            match delete_attempt {
                Ok(delta) => {
                    report.workdirs.actual_free_delta_bytes += delta.max(0);
                    report.workdirs.deleted_items += 1;
                    deleted_bytes += expected;
                }
                Err(exc) => report.add_error(CLEANER, &exc),
            }
        }
        // The release bridge used by pre-persistent agents must keep its old
        // `/tmp/wc-*` path alive through terminal artifact upload. The queue
        // store moves a job out of the live set only after that upload, making
        // the same keep-list a deterministic deletion fence for the symlink.
        // The bounded share reserved above is independent of canonical
        // enumeration: a root full of live jobs must not starve terminal
        // compatibility links forever. The pass never traverses a link or
        // deletes a tree, and total canonical-plus-legacy accounting remains
        // capped at `remaining_scan`.
        let legacy_root = Path::new(LEGACY_WORK_ROOT);
        let mut legacy_remaining = legacy_budget;
        if legacy_root.is_dir() && legacy_remaining > 0 {
            let entries = match std::fs::read_dir(legacy_root) {
                Ok(entries) => entries,
                Err(_) => {
                    report.skip_workdirs("legacy_root_unreadable", 1);
                    return Ok(());
                }
            };
            let legacy_fd = match safefs::open_dir_path(legacy_root) {
                Ok(fd) => fd,
                Err(_) => {
                    report.skip_workdirs("legacy_root_unreadable", 1);
                    return Ok(());
                }
            };
            let legacy_info = safefs::fstat(legacy_fd.as_raw_fd())?;
            for entry in entries {
                if legacy_remaining <= 0 {
                    report.caps.scan = true;
                    report.skip_workdirs("scan_cap", 1);
                    break;
                }
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with(WORKDIR_PREFIX) {
                    continue;
                }
                legacy_remaining -= 1;
                report.workdirs.scanned_items += 1;
                let Some(id) = job_id(&name) else {
                    report.skip_workdirs("not_workdir", 1);
                    continue;
                };
                if live_jobs.iter().any(|live| live == id) {
                    report.skip_workdirs("job_queued_or_running", 1);
                    continue;
                }
                let path = entry.path();
                let info = match std::fs::symlink_metadata(&path) {
                    Ok(info) => info,
                    Err(_) => {
                        report.skip_workdirs("stat_failed", 1);
                        continue;
                    }
                };
                let expected_target = canonical_root.join(format!("{WORKDIR_PREFIX}{id}"));
                let is_bridge = info.file_type().is_symlink()
                    && info.uid() == euid()
                    && path.parent() == Some(legacy_root)
                    && std::fs::read_link(&path).ok().as_deref() == Some(expected_target.as_path());
                // An agent that predates the persistent root left the tree
                // itself here, not a link to it, and no cleaner could see it:
                // this pass only ever unlinked symlinks, and every other
                // cleaner is rooted in the account's home. On 2026-09-04 ten
                // such trees held 14.2 GB on charless-mac-mini while the host
                // sat at 1.1 GB free, which took its object API, the registry
                // authority and every Skarbiec decryption down together while
                // `host reclaim` measured zero in all eight stages. The gate is
                // the canonical pass's own: this account owns it, it is a
                // directory on the legacy root's device, and its job is
                // terminal by the same keep-list.
                let stale_tree = !info.file_type().is_symlink()
                    && info.file_type().is_dir()
                    && info.uid() == euid()
                    && info.dev() == legacy_info.st_dev as u64
                    && path.parent() == Some(legacy_root);
                if !is_bridge && !stale_tree {
                    report.skip_workdirs("not_legacy_bridge", 1);
                    continue;
                }
                report.workdirs.eligible_items += 1;
                let expected = if stale_tree { dir_size(&path) } else { 0 };
                report.workdirs.expected_bytes += expected;
                if policy.mode != "enforce" {
                    continue;
                }
                if report.workdirs.deleted_items >= policy.max_items_per_pass {
                    report.caps.items = true;
                    report.skip_workdirs("item_cap", 1);
                    continue;
                }
                if stale_tree && deleted_bytes >= policy.max_bytes_per_pass {
                    report.caps.bytes = true;
                    report.skip_workdirs("byte_cap", 1);
                    continue;
                }
                let outcome = if stale_tree {
                    (|| -> Result<i64, JanitorError> {
                        let entry_name = entry.file_name();
                        let stat = safefs::fstatat_nofollow(legacy_fd.as_raw_fd(), &entry_name)?;
                        let work_fd = safefs::open_dir_at(legacy_fd.as_raw_fd(), &entry_name)?;
                        if !same_object(&safefs::fstat(work_fd.as_raw_fd())?, &stat) {
                            return Err(JanitorError::os(
                                "queue workdir entry replaced while deleting",
                            ));
                        }
                        let before = free_bytes(home)?;
                        remove_tree_at(
                            legacy_fd.as_raw_fd(),
                            &entry_name,
                            work_fd.as_raw_fd(),
                            legacy_info.st_dev,
                        )?;
                        Ok(free_bytes(home)? - before)
                    })()
                } else {
                    std::fs::remove_file(&path)
                        .map(|()| 0)
                        .map_err(JanitorError::from)
                };
                match outcome {
                    Ok(delta) => {
                        report.workdirs.actual_free_delta_bytes += delta.max(0);
                        report.workdirs.deleted_items += 1;
                        deleted_bytes += expected;
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
