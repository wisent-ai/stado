//! What one tagged cache occupies: the deadline-bounded recursive total of
//! allocated blocks, so the number is comparable with the free space the
//! deletion recovers.

use std::os::fd::{AsRawFd, RawFd};
use std::time::Instant;

use crate::providers::local::disk_cleanup::build_caches::walk::Walk;
use crate::providers::local::disk_cleanup::build_caches::{entry_names, same_object, MAX_DEPTH};
use crate::providers::local::disk_cleanup::{ifmt, safefs, IFDIR};

impl<'a> Walk<'a> {
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
    pub(super) fn tree_bytes(&self, dir_fd: RawFd, depth: usize, complete: &mut bool) -> i64 {
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
}
