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

use std::collections::{BTreeSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

use nix::fcntl::OFlag;
use nix::libc::dev_t;
use nix::sys::stat::{FileStat, Mode};

use super::safefs;
use super::{euid, free_bytes, ifmt, CleanupReport, JanitorError, GIB, IFDIR, IFLNK, IFREG};
use crate::deploy::host_build_caches::CACHEDIR_SIGNATURE;
use crate::targets::{DiskCleanerPolicy, DiskCleanupPolicy};
use serde::{Deserialize, Serialize};

/// Ordinary names stay readable; non-UTF-8 Unix names retain their exact bytes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum CursorPath {
    Text(String),
    Bytes(Vec<u8>),
}

impl CursorPath {
    fn as_path(&self) -> &Path {
        match self {
            Self::Text(path) => Path::new(path),
            Self::Bytes(path) => Path::new(OsStr::from_bytes(path)),
        }
    }
}

impl From<PathBuf> for CursorPath {
    fn from(path: PathBuf) -> Self {
        match path.into_os_string().into_string() {
            Ok(path) => Self::Text(path),
            Err(path) => Self::Bytes(path.into_vec()),
        }
    }
}

impl From<CursorPath> for PathBuf {
    fn from(path: CursorPath) -> Self {
        match path {
            CursorPath::Text(path) => Self::from(path),
            CursorPath::Bytes(path) => Self::from(OsString::from_vec(path)),
        }
    }
}

/// Durable breadth-first work queue for a bounded build-cache walk.
///
/// `frontier[0]` is the directory currently being examined; the remaining
/// paths are directories already discovered but not yet opened. `next_child`
/// is the first entry in `frontier[0]` that has not been examined. Persisting
/// both pieces is what makes a deadline a pause rather than a restart: the
/// next pass opens one parent and continues immediately, without rebuilding
/// every shallower level of the tree.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct BuildCachesCursor {
    version: u8,
    root: CursorPath,
    frontier: VecDeque<CursorPath>,
    next_child: Option<CursorPath>,
}

impl BuildCachesCursor {
    fn fresh(root: PathBuf) -> Self {
        Self {
            version: 1,
            root: root.into(),
            frontier: VecDeque::from([PathBuf::new().into()]),
            next_child: None,
        }
    }

    fn valid_for(&self, root: &Path) -> bool {
        let relative = |path: &Path| {
            path.components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        };
        self.version == 1
            && self.root.as_path() == root
            && !self.frontier.is_empty()
            && self.frontier.iter().all(|path| relative(path.as_path()))
            && self.next_child.as_ref().is_none_or(|path| {
                let path = path.as_path();
                relative(path)
                    && path.file_name().is_some()
                    && path.parent() == self.frontier.front().map(CursorPath::as_path)
            })
    }

    pub(super) fn from_state(state: &serde_json::Value) -> Option<Self> {
        Self::deserialize(state.get("build_caches_cursor")?).ok()
    }

    pub(super) fn pending_directories(&self) -> usize {
        self.frontier.len()
    }

    pub(super) fn resume_label(&self) -> Option<String> {
        self.next_child
            .as_ref()
            .or_else(|| self.frontier.front())
            .map(|path| path.as_path().to_string_lossy().into_owned())
    }
}

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
#[allow(clippy::unnecessary_cast)] // libc field widths differ per OS; the cast is required on macOS
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
/// Cargo's package registry, which this cleaner would otherwise be entitled
/// to delete.
///
/// `~/.cargo/registry` carries a `CACHEDIR.TAG` whose signature is
/// byte-identical to [`CACHEDIR_SIGNATURE`] - cargo writes it itself - so the
/// walk below reads it as a build cache and may evict it. Everything else this
/// cleaner deletes is OUTPUT: a `target/` tree is reproduced by the next
/// build, from inputs already on the disk. The registry is INPUT, shared by
/// every build on the host, and it is not reproduced locally at all: it comes
/// back only by re-fetching from the network, and only if the network answers.
///
/// The failure that matters is not the lost bytes, it is the timing. Deleting
/// it under a running build removes source files that build scripts hold
/// absolute paths to. On 2026-08-31 at 19:15Z the `stado-v0.13.14` train's
/// `Build native Rust control plane` step died exactly that way -
/// `aws-lc-sys` reporting `no such file or directory` for two vendored C files
/// inside this directory, then `ranlib` unable to open the archive it had just
/// written - on the one runner that publishes every release. That extraction
/// verified complete afterwards (2010 of 2010 files), so that particular
/// failure was transient rather than this cleaner's work; the point is that
/// this cleaner was entitled to do it, on that host, in `enforce` mode, with
/// its root defaulting to `$HOME`.
///
/// `CARGO_HOME` is honoured because a build host may move it off the boot
/// volume, which is exactly the kind of host that arms a disk janitor.
fn cargo_registry(home: &Path) -> PathBuf {
    match std::env::var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value).join("registry"),
        _ => home.join(".cargo").join("registry"),
    }
}

fn reserved_roots(home: &Path, policy: &DiskCleanupPolicy) -> Vec<PathBuf> {
    let mut roots = vec![
        home.join(super::STATE_DIR_PARTS[0])
            .join(super::STATE_DIR_PARTS[1]),
        home.join(".cache").join("huggingface").join("hub"),
        cargo_registry(home),
    ];
    // A CLI running out of a tagged build tree must not unlink its own
    // executable while it is restoring the host's ability to do work.
    if let Ok(executable) = std::env::current_exe() {
        roots.push(executable);
    }
    let configured_root = |name: &str, default: &[&str]| -> PathBuf {
        match policy.cleaners.get(name).and_then(|c| c.root.as_deref()) {
            Some(root) => crate::config_file::expand_tilde(root),
            None => default
                .iter()
                .fold(home.to_path_buf(), |path, part| path.join(part)),
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
    /// Directories discovered but not yet fully examined, in breadth-first
    /// order. Unlike the former positional cursor, this queue is durable:
    /// resuming never has to reconstruct old levels before doing new work.
    frontier: VecDeque<CursorPath>,
    /// First unexamined child of the directory at the front of `frontier`.
    /// `None` means that parent has not been started.
    next_child: Option<PathBuf>,
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
    ///
    /// Bounded by the pass deadline, and never charged to `scanned_items`.
    /// The tag is the build tool's statement that this directory is ONE
    /// regenerable unit, so it is one scanned candidate however many files
    /// it holds — a `target/` is one deletable thing, not the hundred
    /// thousand entries inside it. It used to be neither: the recursion was
    /// unbounded and free, so the first eligible cache could consume the
    /// whole thirty-second deadline totalling bytes, and the pass then
    /// halted with `caps.deadline` set and no cache examined after it.
    ///
    /// `complete` is cleared when the deadline cut the total short. The
    /// bytes returned are then the bytes actually proven, never an estimate
    /// of the rest: [`Walk::judge`] reports the shortfall as
    /// `scan_deadline`, so a partial total reads as "I did not finish
    /// looking" instead of as a smaller true-looking number.
    fn tree_bytes(&self, dir_fd: RawFd, depth: usize, complete: &mut bool) -> i64 {
        if Instant::now() >= self.deadline {
            *complete = false;
            return 0;
        }
        let Ok(names) = entry_names(dir_fd) else {
            return 0;
        };
        let mut total = 0i64;
        for name in names {
            if Instant::now() >= self.deadline {
                *complete = false;
                return total;
            }
            let Ok(info) = safefs::fstatat_nofollow(dir_fd, &name) else {
                continue;
            };
            #[allow(clippy::unnecessary_cast)] // st_mode is u16 on macOS, u32 on Linux
            if ifmt(info.st_mode as u32) == IFDIR && info.st_dev == self.root_dev {
                if depth + 1 > MAX_DEPTH {
                    continue;
                }
                if let Ok(child) = safefs::open_dir_at(dir_fd, &name) {
                    if same_object(&safefs::fstat(child.as_raw_fd()).unwrap_or(info), &info) {
                        total += self.tree_bytes(child.as_raw_fd(), depth + 1, complete);
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
        let mut complete = true;
        let expected = self.tree_bytes(dir_fd, 0, &mut complete);
        report.builds.expected_bytes += expected;
        if !complete {
            // The cache is eligible and its bytes are the bytes proven, so
            // both are reported — but the pass has to say that the number is
            // a floor and not the total, or an operator reads a short
            // `expected_bytes` as the whole of what a cleanup would recover.
            report.caps.deadline = true;
            report.skip_builds("scan_deadline", 1);
        }
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

    /// Examine the tree one level at a time, stopping at every directory its
    /// own build tool tagged as regenerable.
    ///
    /// LEVEL ORDER, and that is the whole of why this cleaner now finds
    /// anything. A build tool writes its cache at the TOP of the tree it
    /// generates — that is where the standard puts `CACHEDIR.TAG` — so every
    /// candidate is shallow and everything deep is some tree's contents. A
    /// depth-first walk spends its budget the other way round: on
    /// `lukasz-macbook` on 2026-09-02, crossing the declared root took
    /// 803,825 directories against a `max_scan_items` of 100,000, and the
    /// walk was still inside the first repository's `node_modules` — 7,297
    /// directories deep into one alphabetically-first branch — having
    /// examined none of the 174 tagged caches the tree holds, including a
    /// 62 GiB `target/`. Visiting by level reaches 41 of them in 5,808
    /// charges and 107 in 38,578, all of them inside one pass's budget, on
    /// the same tree with the same cap.
    ///
    /// The order changes only WHICH directories a bounded pass gets to look
    /// at. Every deletion criterion — the tag, the age, the reserved roots,
    /// the ownership and device checks — is applied exactly as before.
    fn walk_levels(
        &mut self,
        root_fd: RawFd,
        root: &Path,
        report: &mut CleanupReport,
    ) -> Result<Progress, JanitorError> {
        while let Some(parent) = self.frontier.pop_front().map(PathBuf::from) {
            if Instant::now() >= self.deadline {
                self.frontier.push_front(parent.into());
                report.caps.deadline = true;
                report.skip_builds("scan_deadline", 1);
                return Ok(Progress::Halt);
            }
            let depth = parent.components().count();
            if depth >= MAX_DEPTH {
                self.next_child = None;
                report.skip_builds("depth_cap", 1);
                continue;
            }
            // A durable queue is only a location hint. Reopen and revalidate
            // every component against today's tree and policy before using it.
            let parent_fd = (|| -> Result<std::os::fd::OwnedFd, JanitorError> {
                let mut descriptor = safefs::dup_fd(root_fd)?;
                let mut absolute = root.to_path_buf();
                for part in parent.components() {
                    absolute.push(part);
                    let child = safefs::open_dir_at(descriptor.as_raw_fd(), part.as_os_str())?;
                    let info = safefs::fstat(child.as_raw_fd())?;
                    if info.st_uid != euid()
                        || info.st_dev != self.root_dev
                        || self.reserved.iter().any(|item| absolute.starts_with(item))
                    {
                        return Err(JanitorError::os("queued directory is no longer safe"));
                    }
                    let guards_reserved =
                        self.reserved.iter().any(|item| item.starts_with(&absolute));
                    if !guards_reserved && matches!(self.read_tag(child.as_raw_fd())?, Tag::Signed)
                    {
                        return Err(JanitorError::os("queued ancestor is now a tagged cache"));
                    }
                    descriptor = child;
                }
                Ok(descriptor)
            })();
            let parent_fd = match parent_fd {
                Ok(descriptor) => descriptor,
                Err(_) => {
                    self.next_child = None;
                    report.skip_builds("entry_replaced", 1);
                    continue;
                }
            };
            match self.examine(parent_fd.as_raw_fd(), root, &parent, report) {
                Ok(Progress::Continue) => self.next_child = None,
                result => {
                    self.frontier.push_front(parent.into());
                    return result;
                }
            }
        }
        Ok(Progress::Continue)
    }

    /// Examine every child of one frontier directory: judge the ones their
    /// build tool tagged, and hand the rest to the next level.
    fn examine(
        &mut self,
        parent_fd: RawFd,
        root: &Path,
        parent: &Path,
        report: &mut CleanupReport,
    ) -> Result<Progress, JanitorError> {
        let names = match entry_names(parent_fd) {
            Ok(names) => names,
            Err(_) => {
                report.skip_builds("stat_failed", 1);
                return Ok(Progress::Continue);
            }
        };
        let first = self
            .next_child
            .take()
            .and_then(|path| path.file_name().map(OsStr::to_os_string))
            .unwrap_or_default();
        for name in names.range(first..) {
            let relative = parent.join(name);
            if Instant::now() >= self.deadline {
                report.caps.deadline = true;
                report.skip_builds("scan_deadline", 1);
                self.next_child = Some(relative);
                return Ok(Progress::Halt);
            }
            let info = match safefs::fstatat_nofollow(parent_fd, name) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    continue;
                }
            };
            let kind = ifmt(info.st_mode as u32);
            if kind == IFLNK {
                report.skip_builds("symlink_not_followed", 1);
                continue;
            }
            if kind != IFDIR {
                continue;
            }
            let absolute = root.join(&relative);
            if self.reserved.iter().any(|item| absolute.starts_with(item)) {
                report.skip_builds("reserved_or_hidden", 1);
                continue;
            }
            if let Progress::Halt = self.charge(report) {
                self.next_child = Some(relative);
                return Ok(Progress::Halt);
            }
            let child = match safefs::open_dir_at(parent_fd, name) {
                Ok(child) => child,
                Err(exc)
                    if matches!(
                        exc.raw_os_error(),
                        Some(nix::libc::ELOOP) | Some(nix::libc::ENOTDIR)
                    ) =>
                {
                    report.skip_builds("entry_replaced", 1);
                    continue;
                }
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    continue;
                }
            };
            let child_info = match safefs::fstat(child.as_raw_fd()) {
                Ok(info) => info,
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    continue;
                }
            };
            if !same_object(&child_info, &info) {
                report.skip_builds("entry_replaced", 1);
                continue;
            }
            if child_info.st_uid != euid() || child_info.st_dev != self.root_dev {
                report.skip_builds("unsafe_owner_or_device", 1);
                continue;
            }
            let guards_reserved = self.reserved.iter().any(|item| item.starts_with(&absolute));
            let tag = match self.read_tag(child.as_raw_fd()) {
                Ok(tag) => tag,
                Err(_) => {
                    report.skip_builds("stat_failed", 1);
                    Tag::Absent
                }
            };
            match tag {
                Tag::Signed if guards_reserved => {
                    report.skip_builds("reserved_or_hidden", 1);
                    self.frontier.push_back(relative.into());
                }
                // Tagged caches remain indivisible candidates, never parents
                // whose contents can be independently selected for deletion.
                Tag::Signed => {
                    match self.judge(parent_fd, name, child.as_raw_fd(), &child_info, report) {
                        Ok(Progress::Continue) => {}
                        result => {
                            self.next_child = Some(relative);
                            return result;
                        }
                    }
                }
                Tag::Unsigned => {
                    report.skip_builds("untagged", 1);
                    self.frontier.push_back(relative.into());
                }
                Tag::Absent => self.frontier.push_back(relative.into()),
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
            return Err(JanitorError::os(
                "build cache entry replaced while deleting",
            ));
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
///
/// The durable frontier contains the unvisited directories, not merely the
/// position of the last visit. Older positional cursors restart once to build
/// this queue; subsequent passes open the next parent directly. Replaying all
/// prior levels used to consume the entire deadline with zero newly scanned
/// directories on every pass.
///
/// Neither the order nor the cursor changes WHICH directories may be
/// deleted. Every criterion — the tag, the age, the reserved roots, the
/// ownership and device checks — is applied exactly as it would be on a walk
/// that started at the root and went straight down.
pub(super) fn scan_build_caches(
    home: &Path,
    policy: &DiskCleanupPolicy,
    now: f64,
    remaining_scan: i64,
    deadline: Instant,
    cursor: Option<BuildCachesCursor>,
    report: &mut CleanupReport,
) {
    // A pass that declines to walk must preserve its existing checkpoint.
    report.builds_cursor = cursor;
    let Some(configured) = policy.cleaners.get("build_caches") else {
        return;
    };
    if remaining_scan <= 0 {
        return;
    }
    let body = |report: &mut CleanupReport| -> Result<(), JanitorError> {
        let root = match &configured.root {
            Some(configured_root) => {
                let expanded = crate::config_file::expand_tilde(configured_root);
                if !expanded.is_dir() {
                    report.skip_builds("root_absent", 1);
                    return Ok(());
                }
                std::fs::canonicalize(&expanded)?
            }
            None => home.to_path_buf(),
        };
        let root_fd = safefs::open_dir_path(&root)?;
        let root_info = safefs::fstat(root_fd.as_raw_fd())?;
        if root_info.st_uid != euid() {
            return Err(JanitorError::os("build cache root ownership mismatch"));
        }
        let cursor = report
            .builds_cursor
            .take()
            .filter(|cursor| cursor.valid_for(&root))
            .unwrap_or_else(|| BuildCachesCursor::fresh(root.clone()));
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
            frontier: cursor.frontier,
            next_child: cursor.next_child.map(PathBuf::from),
        };
        // The root itself is never a candidate. Its queued children are
        // revalidated through directory descriptors before they are used.
        let result = walk.walk_levels(root_fd.as_raw_fd(), &root, report);
        report.builds_cursor = if walk.frontier.is_empty() {
            None
        } else {
            Some(BuildCachesCursor {
                version: 1,
                root: root.into(),
                frontier: walk.frontier,
                next_child: walk.next_child.map(CursorPath::from),
            })
        };
        report.builds_resume_from = report
            .builds_cursor
            .as_ref()
            .and_then(BuildCachesCursor::resume_label);
        result.map(|_| ())
    };
    if let Err(exc) = body(report) {
        report.add_error("build_caches", &exc);
    }
}
