//! Build-cache cleanup: eviction of directories their own build tool has
//! declared regenerable, by the Cache Directory Tagging Standard.
//!
//! NO Python original. This cleaner exists because the janitor's two
//! consumers, `huggingface_cache` and `weles_recordings`, together describe
//! almost nothing of what actually fills a developer host. The operator's
//! laptop reached 8.8 GB free of 1.8 TB while carrying roughly 450 GB of
//! build and scratch trees — 620 GB of them across the checked-out
//! repositories at the worst point — and `disk-cleanup` had nothing to say
//! about any of it: the host's policy was `mode: "off"`, and even armed it
//! would have reported a healthy no-op, because not one of those directories
//! belongs to a cleaner it knows. A janitor that reports "nothing to do" on a
//! disk that is 99.5% full is worse than no janitor, because the number is
//! believed.
//!
//! `stado host build-caches` ([`crate::deploy::host_build_caches`]) already
//! recognises such a directory safely, but only when an operator asks it to,
//! over ssh, one host at a time. This module is the same judgement inside the
//! automatic pass.
//!
//! The safety criterion is that module's, unchanged and imported rather than
//! copied: a directory may be deleted if and only if it contains a
//! `CACHEDIR.TAG` whose first line is
//! [`host_build_caches::CACHEDIR_SIGNATURE`]. Cargo — and cmake, and many
//! others — write that file precisely so a cleaner may remove the directory
//! without asking. No directory-name matching, no extension lists: the tool
//! that produced the bytes is the only party that gets to say they are
//! reproducible.
//!
//! Unlike [`super::weles`], which is a path-based port, every operation here
//! is dir-fd-relative through [`super::safefs`], as in [`super::hf`]. The
//! scan root is the whole of `$HOME` by default, which is the one root in the
//! janitor no one can enumerate in advance; with plain paths, a symlink
//! swapped in mid-walk anywhere in that tree would be a recursive delete of
//! whatever it pointed at.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::Instant;

use nix::fcntl::OFlag;
use nix::libc::dev_t;
use nix::sys::stat::{FileStat, Mode};

use super::safefs;
use super::{euid, free_bytes, ifmt, CleanupReport, JanitorError, GIB, IFDIR, IFLNK, IFREG};
use crate::deploy::host_build_caches::CACHEDIR_SIGNATURE;
use crate::targets::{DiskCleanerPolicy, DiskCleanupPolicy};

/// The standard's file name; the signature lives in its first line.
const TAG_NAME: &str = "CACHEDIR.TAG";

/// Enough bytes to hold the signature line and prove where it ends. The
/// standard puts the signature at offset 0, so a longer read would only
/// widen what an attacker's file can say.
const TAG_READ_BYTES: usize = 64;

/// One open directory per level is held while the walk is inside it, so the
/// depth limit is also the fd budget. 64 is far below any macOS descriptor
/// limit and far above any real build tree: a `target/` is five or six deep,
/// and the deepest `node_modules` chains npm still produces are around
/// thirty.
const MAX_DEPTH: usize = 64;

/// What `CACHEDIR.TAG` says about the directory holding it.
enum Tag {
    /// Present, a regular file we own, first line is the signature.
    Signed,
    /// Present but not a valid tag: wrong first line, a symlink, a
    /// directory, or owned by someone else.
    Unsigned,
    /// No tag file at all — an ordinary directory on the way down.
    Absent,
}

/// Whether the walk may continue at all, or has spent a pass-wide budget.
enum Progress {
    Continue,
    /// Scan cap, deadline, or the free-space target: the pass is done and
    /// every remaining directory is left unexamined rather than half-judged.
    Halt,
}

/// The one identity comparison this module needs: a directory opened with
/// `O_NOFOLLOW` must be the exact object the preceding `fstatat` described,
/// or something replaced it between the two calls.
fn same_object(first: &FileStat, second: &FileStat) -> bool {
    first.st_dev == second.st_dev
        && first.st_ino == second.st_ino
        && ifmt(first.st_mode as u32) == ifmt(second.st_mode as u32)
}

/// Roots the build-cache cleaner must never delete even when they carry a
/// valid tag, plus (by prefix) everything beneath them.
///
/// This is not defensive decoration. Several tools tag `~/.cache` itself,
/// and `~/.cache/wisent-compute` holds the janitor's own state file and the
/// lock this very pass is holding open; `~/.cache/huggingface/hub` and the
/// weles recordings root belong to cleaners whose deletion rules are
/// stricter than a tag file (blob reference counts, durable upload proof).
/// A single tag file dropped one level above them would otherwise let the
/// youngest cleaner in the janitor overrule both of the older ones.
fn reserved_roots(home: &Path, policy: &DiskCleanupPolicy) -> Vec<PathBuf> {
    let mut roots = vec![
        home.join(super::STATE_DIR_PARTS[0])
            .join(super::STATE_DIR_PARTS[1]),
        home.join(".cache").join("huggingface").join("hub"),
    ];
    let configured_root = |name: &str, default: &[&str]| -> PathBuf {
        match policy.cleaners.get(name).and_then(|c| c.root.as_deref()) {
            Some(root) => crate::config_file::expand_tilde(root),
            None => default.iter().fold(home.to_path_buf(), |path, part| {
                path.join(part)
            }),
        }
    };
    roots.push(configured_root(
        "weles_recordings",
        &["weles", "recordings"],
    ));
    if let Some(hf_root) = policy
        .cleaners
        .get("huggingface_cache")
        .and_then(|c| c.root.as_deref())
    {
        roots.push(crate::config_file::expand_tilde(hf_root));
    }
    roots
}

/// The scan's shared state: the budgets it spends and the root it may not
/// leave.
struct Walk<'a> {
    home: &'a Path,
    policy: &'a DiskCleanupPolicy,
    configured: &'a DiskCleanerPolicy,
    /// Epoch seconds the pass started (Python-style `time.time()`).
    now: f64,
    deadline: Instant,
    /// Directories left in this pass's share of `max_scan_items`.
    remaining_scan: i64,
    /// `st_dev` of the scan root. A mount point inside the tree is refused
    /// rather than descended: an external disk or a network share mounted
    /// under a tagged directory is not what the build tool declared
    /// regenerable.
    root_dev: dev_t,
    reserved: Vec<PathBuf>,
    /// Bytes this pass expects to have freed, against `max_bytes_per_pass`.
    deleted_bytes: i64,
}

impl<'a> Walk<'a> {
    /// Age gate. The directory's own mtime is what
    /// `find "$dir" -maxdepth 0 -mtime +N` tests in the remote script.
    fn old_enough(&self, info: &FileStat) -> bool {
        (info.st_mtime as f64) <= self.now - self.configured.min_age_seconds as f64
    }

    /// Charge one directory to the scan budget, or report which pass-wide
    /// limit stopped the walk.
    ///
    /// The deadline is checked first because it is the only limit that can
    /// be hit by a tree the cleaner has not finished reading: `$HOME` on a
    /// developer machine holds millions of directories, so "how many did we
    /// look at" is not on its own a bound on how long we looked.
    fn charge(&mut self, report: &mut CleanupReport) -> Progress {
        if Instant::now() >= self.deadline {
            report.caps.deadline = true;
            report.skip_builds("scan_deadline", 1);
            return Progress::Halt;
        }
        if self.remaining_scan <= 0 {
            report.caps.scan = true;
            report.skip_builds("scan_cap", 1);
            return Progress::Halt;
        }
        self.remaining_scan -= 1;
        report.builds.scanned_items += 1;
        Progress::Continue
    }

    /// Read `CACHEDIR.TAG` in the directory `dir_fd` names.
    fn read_tag(&self, dir_fd: RawFd) -> Result<Tag, JanitorError> {
        let name = OsStr::new(TAG_NAME);
        let info = match safefs::fstatat_nofollow(dir_fd, name) {
            Ok(info) => info,
            Err(exc) if exc.kind() == io::ErrorKind::NotFound => return Ok(Tag::Absent),
            Err(exc) => return Err(exc.into()),
        };
        // A symlinked or directory "tag", or one owned by another user, is
        // not the build tool's statement about this directory — it is
        // somebody else's, and it authorizes nothing.
        if ifmt(info.st_mode as u32) != IFREG
            || info.st_uid != euid()
            || info.st_dev != self.root_dev
        {
            return Ok(Tag::Unsigned);
        }
        let descriptor = safefs::open_file_at(dir_fd, name, OFlag::O_RDONLY, Mode::empty())?;
        let opened = safefs::fstat(descriptor.as_raw_fd())?;
        if !same_object(&opened, &info) {
            return Ok(Tag::Unsigned);
        }
        let payload = safefs::read_fd(descriptor.as_raw_fd(), TAG_READ_BYTES)?;
        let first_line = payload
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default();
        let first_line = first_line.strip_suffix(b"\r").unwrap_or(first_line);
        if first_line == CACHEDIR_SIGNATURE.as_bytes() {
            Ok(Tag::Signed)
        } else {
            Ok(Tag::Unsigned)
        }
    }

    /// Recursively total the bytes a tagged directory occupies, as `du -sk`
    /// does in the remote script: allocated blocks, not apparent sizes, so
    /// the number is comparable with the free space the deletion recovers.
    /// Unreadable subtrees contribute nothing rather than aborting the
    /// estimate — the deletion below reports its own failures.
    fn tree_bytes(&self, dir_fd: RawFd, depth: usize) -> i64 {
        let Ok(names) = entry_names(dir_fd) else {
            return 0;
        };
        let mut total = 0i64;
        for name in names {
            let Ok(info) = safefs::fstatat_nofollow(dir_fd, &name) else {
                continue;
            };
            if ifmt(info.st_mode as u32) == IFDIR && info.st_dev == self.root_dev {
                if depth + 1 > MAX_DEPTH {
                    continue;
                }
                if let Ok(child) = safefs::open_dir_at(dir_fd, &name) {
                    if same_object(&safefs::fstat(child.as_raw_fd()).unwrap_or(info), &info) {
                        total += self.tree_bytes(child.as_raw_fd(), depth + 1);
                    }
                }
                continue;
            }
            total += info.st_blocks * 512;
        }
        total
    }

    /// Judge one directory whose tag is valid, and — in `enforce` — remove
    /// it. Never descends: a cache nested in a cache needs no special case,
    /// because the parent is reported and removed whole, so the child must
    /// not be counted a second time.
    fn judge(
        &mut self,
        parent_fd: RawFd,
        name: &OsStr,
        dir_fd: RawFd,
        info: &FileStat,
        report: &mut CleanupReport,
    ) -> Result<Progress, JanitorError> {
        if !self.old_enough(info) {
            report.skip_builds("too_young", 1);
            return Ok(Progress::Continue);
        }
        report.builds.eligible_items += 1;
        let expected = self.tree_bytes(dir_fd, 0);
        report.builds.expected_bytes += expected;
        if self.policy.mode != "enforce" {
            return Ok(Progress::Continue);
        }
        if report.builds.deleted_items >= self.policy.max_items_per_pass {
            report.caps.items = true;
            report.skip_builds("item_cap", 1);
            return Ok(Progress::Continue);
        }
        if self.deleted_bytes >= self.policy.max_bytes_per_pass {
            report.caps.bytes = true;
            report.skip_builds("byte_cap", 1);
            return Ok(Progress::Continue);
        }
        // Enough was recovered: stop deleting. The pass computes the
        // outcome from free space afterwards, but nothing above this call
        // stops the scan, so the cleaner has to.
        if free_bytes(self.home)? >= self.policy.target_free_gb * GIB {
            return Ok(Progress::Halt);
        }
        let before = free_bytes(self.home)?;
        match remove_tree(parent_fd, name, dir_fd, self.root_dev) {
            Ok(()) => {
                let after = free_bytes(self.home)?;
                report.builds.actual_free_delta_bytes += (after - before).max(0);
                report.builds.deleted_items += 1;
                self.deleted_bytes += expected;
            }
            // A partially removed tree is reported, not retried: whatever
            // refused (an unwritable subdirectory, a vanished entry, a
            // replaced one) will be judged again next pass on what is left.
            Err(exc) => report.add_error("build_caches", &exc),
        }
        Ok(Progress::Continue)
    }

    /// Examine every child of one directory, descending until a valid tag
    /// says "this whole subtree is regenerable".
    fn descend(
        &mut self,
        dir_fd: RawFd,
        dir_path: &Path,
        depth: usize,
        report: &mut CleanupReport,
    ) -> Result<Progress, JanitorError> {
        for name in entry_names(dir_fd)? {
            let info = match safefs::fstatat_nofollow(dir_fd, &name) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    continue;
                }
            };
            let kind = ifmt(info.st_mode as u32);
            if kind == IFLNK {
                // Never followed, never a candidate, and reported so an
                // operator can see the cleaner declined rather than missed
                // it. `O_NOFOLLOW` below would refuse it anyway; saying so
                // here is what makes the refusal visible.
                report.skip_builds("symlink_not_followed", 1);
                continue;
            }
            if kind != IFDIR {
                continue;
            }
            let child_path = dir_path.join(&name);
            if self.reserved.iter().any(|root| child_path.starts_with(root)) {
                report.skip_builds("reserved_or_hidden", 1);
                continue;
            }
            if let Progress::Halt = self.charge(report) {
                return Ok(Progress::Halt);
            }
            if depth + 1 > MAX_DEPTH {
                report.skip_builds("depth_cap", 1);
                continue;
            }
            let child = match safefs::open_dir_at(dir_fd, &name) {
                Ok(child) => child,
                Err(exc)
                    if matches!(
                        exc.raw_os_error(),
                        Some(nix::libc::ELOOP) | Some(nix::libc::ENOTDIR)
                    ) =>
                {
                    // `fstatat` said directory, the `O_NOFOLLOW` open says
                    // symlink or non-directory: the name was swapped
                    // between the two calls.
                    report.skip_builds("entry_replaced", 1);
                    continue;
                }
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    continue;
                }
            };
            let child_info = safefs::fstat(child.as_raw_fd())?;
            if !same_object(&child_info, &info) {
                report.skip_builds("entry_replaced", 1);
                continue;
            }
            if child_info.st_uid != euid() || child_info.st_dev != self.root_dev {
                report.skip_builds("unsafe_owner_or_device", 1);
                continue;
            }
            // A directory that CONTAINS a reserved root may still hide
            // build caches deeper down, so it is descended — but its own
            // tag can never authorize deleting it, because that would take
            // the reserved root with it.
            let guards_reserved = self.reserved.iter().any(|root| root.starts_with(&child_path));
            let tag = match self.read_tag(child.as_raw_fd()) {
                Ok(tag) => tag,
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    Tag::Absent
                }
            };
            let progress = match tag {
                Tag::Signed if guards_reserved => {
                    report.skip_builds("reserved_or_hidden", 1);
                    self.descend(child.as_raw_fd(), &child_path, depth + 1, report)?
                }
                Tag::Signed => self.judge(dir_fd, &name, child.as_raw_fd(), &child_info, report)?,
                // A tag file whose first line is not the signature is not a
                // permission. Reported, and the walk continues downward:
                // the caches below it are unaffected by a stray file above.
                Tag::Unsigned => {
                    report.skip_builds("untagged", 1);
                    self.descend(child.as_raw_fd(), &child_path, depth + 1, report)?
                }
                Tag::Absent => self.descend(child.as_raw_fd(), &child_path, depth + 1, report)?,
            };
            if let Progress::Halt = progress {
                return Ok(Progress::Halt);
            }
        }
        Ok(Progress::Continue)
    }
}

/// One `os.scandir` worth of names, with the two self-references dropped and
/// a deterministic order, so two passes over an unchanged tree spend the
/// scan budget on the same directories.
fn entry_names(dir_fd: RawFd) -> Result<BTreeSet<OsString>, JanitorError> {
    let mut names = BTreeSet::new();
    for name in safefs::DirEntries::open(dir_fd)? {
        let name = name?;
        if name == "." || name == ".." {
            continue;
        }
        names.insert(name);
    }
    Ok(names)
}

/// Delete the tree `name` names beneath `parent_fd`, contents first.
///
/// `dir_fd` is the already-validated descriptor for that same directory, so
/// the contents are removed through a handle no rename can redirect. Only
/// the final `rmdir` addresses the name again, and it removes a directory
/// only when empty — the worst a swap at that instant can do is fail.
fn remove_tree(
    parent_fd: RawFd,
    name: &OsStr,
    dir_fd: RawFd,
    root_dev: dev_t,
) -> Result<(), JanitorError> {
    remove_contents(dir_fd, root_dev, 0)?;
    safefs::rmdir_at(parent_fd, name)?;
    Ok(())
}

/// Unlink everything inside `dir_fd`, depth first.
fn remove_contents(dir_fd: RawFd, root_dev: dev_t, depth: usize) -> Result<(), JanitorError> {
    for name in entry_names(dir_fd)? {
        let info = safefs::fstatat_nofollow(dir_fd, &name)?;
        // Symlinks, sockets, devices: unlinked as names, never traversed.
        if ifmt(info.st_mode as u32) != IFDIR {
            safefs::unlink_at(dir_fd, &name)?;
            continue;
        }
        if info.st_dev != root_dev {
            return Err(JanitorError::os("build cache spans a device boundary"));
        }
        if depth + 1 > MAX_DEPTH {
            return Err(JanitorError::os("build cache nested too deep to remove"));
        }
        let child = safefs::open_dir_at(dir_fd, &name)?;
        if !same_object(&safefs::fstat(child.as_raw_fd())?, &info) {
            return Err(JanitorError::os("build cache entry replaced while deleting"));
        }
        remove_contents(child.as_raw_fd(), root_dev, depth + 1)?;
        drop(child);
        safefs::rmdir_at(dir_fd, &name)?;
    }
    Ok(())
}

/// Scan the build-cache root and evict every directory its own build tool
/// tagged as regenerable.
///
/// `remaining_scan` is this cleaner's share of `max_scan_items` left by the
/// cleaners that ran before it, and `deadline` is the pass deadline the HF
/// scan also honours: unlike the other two roots, this one can be the whole
/// of `$HOME`, where the walk — not the deletion — is the expensive half.
pub fn scan_build_caches(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    report: &mut CleanupReport,
) {
    let Some(configured) = policy.cleaners.get("build_caches") else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        // Default root is `$HOME` itself: build trees live wherever the
        // operator checked the repository out, and no fixed subdirectory of
        // home describes that. `root` narrows the walk for a host whose
        // home is too large to cross within one deadline.
        let root = match &configured.root {
            Some(configured_root) => {
                let expanded = crate::config_file::expand_tilde(configured_root);
                if !expanded.is_dir() {
                    report.skip_builds("root_absent", 1);
                    return Ok(());
                }
                // Resolved, because the reserved roots below are named
                // relative to the resolved home: a root spelled through a
                // symlink (`/tmp/...` for `/private/tmp/...` on macOS) would
                // otherwise compare unequal to a reserved path it contains,
                // and the guard that keeps the janitor's own state dir alive
                // would silently stop matching.
                std::fs::canonicalize(&expanded)?
            }
            None => home.to_path_buf(),
        };
        let root_fd = safefs::open_dir_path(&root)?;
        let root_info = safefs::fstat(root_fd.as_raw_fd())?;
        if root_info.st_uid != euid() {
            return Err(JanitorError::os("build cache root ownership mismatch"));
        }
        let mut walk = Walk {
            home,
            policy,
            configured,
            now,
            deadline,
            remaining_scan,
            root_dev: root_info.st_dev,
            reserved: reserved_roots(home, policy),
            deleted_bytes: 0,
        };
        // The root itself is never a candidate, tagged or not: the cleaner
        // deletes caches inside the root it was given, and a root it
        // deleted would leave the next pass reporting `root_absent` about a
        // home directory.
        walk.descend(root_fd.as_raw_fd(), &root, 0, report)?;
        Ok(())
    };
    if let Err(exc) = body(report) {
        report.add_error("build_caches", &exc);
    }
}
