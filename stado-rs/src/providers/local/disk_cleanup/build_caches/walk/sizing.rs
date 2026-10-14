//! Allocated blocks observed inside one tagged cache. This is not a promise
//! of recovered filesystem space.

use std::ffi::OsStr;
use std::os::fd::{AsRawFd, RawFd};

use crate::providers::local::disk_cleanup::build_caches::walk::Walk;
use crate::providers::local::disk_cleanup::build_caches::{entry_names, same_object};
use crate::providers::local::disk_cleanup::{ifmt, safefs, JanitorError, IFDIR};

fn entry_error(operation: &str, name: &OsStr, mut error: JanitorError) -> JanitorError {
    error.message = format!("{operation} {name:?}: {}", error.message);
    error
}

impl<'a> Walk<'a> {
    /// Measure one regenerable unit completely before using its size.
    /// An unreadable or changed subtree is an error, not a zero-byte estimate.
    pub(super) fn tree_bytes(&self, dir_fd: RawFd, depth: usize) -> Result<i64, JanitorError> {
        let names = entry_names(dir_fd).map_err(|mut error| {
            error.message = format!("read cache entries at depth {depth}: {}", error.message);
            error
        })?;
        let mut total = 0i64;
        for name in names {
            let info = safefs::fstatat_nofollow(dir_fd, &name)
                .map_err(|error| entry_error("stat cache entry", &name, error.into()))?;
            #[allow(clippy::unnecessary_cast)]
            if ifmt(info.st_mode as u32) == IFDIR {
                if info.st_dev != self.root_dev {
                    return Err(JanitorError::os(&format!(
                        "cache directory {name:?} is on device {}, expected {}",
                        info.st_dev, self.root_dev
                    )));
                }
                let child = safefs::open_dir_at(dir_fd, &name)
                    .map_err(|error| entry_error("open cache directory", &name, error.into()))?;
                let opened = safefs::fstat(child.as_raw_fd()).map_err(|error| {
                    entry_error("stat opened cache directory", &name, error.into())
                })?;
                if !same_object(&opened, &info) {
                    return Err(JanitorError::os(&format!(
                        "cache directory {name:?} changed during measurement: expected inode {}, observed {}",
                        info.st_ino, opened.st_ino
                    )));
                }
                total += self
                    .tree_bytes(child.as_raw_fd(), depth + 1)
                    .map_err(|error| entry_error("measure cache subtree", &name, error))?;
            } else {
                total += info.st_blocks * 512;
            }
        }
        Ok(total)
    }
}
