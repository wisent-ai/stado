//! Scratch traversal uses the janitor's non-following directory descriptors.
//! One descriptor is held per level; a tree deeper than the process may open
//! reports the system's own refusal.

use crate::providers::local::disk_cleanup::safefs;
use nix::sys::stat::FileStat;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::Path;

pub(super) fn open_root(home: &Path) -> io::Result<Option<OwnedFd>> {
    let mut fd = safefs::open_dir_path(home)?;
    for name in [".stado", "work"] {
        match safefs::open_dir_at(fd.as_raw_fd(), OsStr::new(name)) {
            Ok(child) => fd = child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(Some(fd))
}

pub(super) fn names(fd: RawFd) -> io::Result<Vec<OsString>> {
    let mut entries = safefs::DirEntries::open(fd)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

fn same(first: &FileStat, second: &FileStat) -> bool {
    first.st_dev == second.st_dev
        && first.st_ino == second.st_ino
        && first.st_mode == second.st_mode
        && first.st_uid == second.st_uid
}

fn checked_child(parent: RawFd, name: &OsStr, info: &FileStat) -> io::Result<OwnedFd> {
    let parent_info = safefs::fstat(parent)?;
    if info.st_dev != parent_info.st_dev {
        return Err(io::Error::other(
            "working directory crosses a filesystem boundary",
        ));
    }
    let child = safefs::open_dir_at(parent, name)?;
    if !same(info, &safefs::fstat(child.as_raw_fd())?) {
        return Err(io::Error::other(
            "working directory changed during inspection",
        ));
    }
    Ok(child)
}

pub(super) struct Tree {
    pub name: OsString,
    fd: OwnedFd,
    info: FileStat,
}

impl Tree {
    pub fn open(parent: RawFd, name: OsString, info: FileStat) -> io::Result<Self> {
        let fd = checked_child(parent, &name, &info)?;
        Ok(Self { name, fd, info })
    }

    pub fn bytes(&self) -> io::Result<i64> {
        measure(self.fd.as_raw_fd())
    }

    pub fn remove(self, parent: RawFd) -> io::Result<()> {
        if !same(&self.info, &safefs::fstatat_nofollow(parent, &self.name)?) {
            return Err(io::Error::other(
                "working directory replaced before removal",
            ));
        }
        remove_contents(self.fd.as_raw_fd())?;
        if !same(&self.info, &safefs::fstatat_nofollow(parent, &self.name)?) {
            return Err(io::Error::other(
                "working directory replaced during removal",
            ));
        }
        safefs::rmdir_at(parent, &self.name)
    }
}

fn measure(fd: RawFd) -> io::Result<i64> {
    let mut bytes = 0i64;
    for name in names(fd)? {
        let info = safefs::fstatat_nofollow(fd, &name)?;
        let size = if info.st_mode & nix::libc::S_IFMT == nix::libc::S_IFDIR {
            let child = checked_child(fd, &name, &info)?;
            measure(child.as_raw_fd())?
        } else {
            info.st_size.max(0)
        };
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| io::Error::other("working directory size overflow"))?;
    }
    Ok(bytes)
}

fn remove_contents(fd: RawFd) -> io::Result<()> {
    for name in names(fd)? {
        let info = safefs::fstatat_nofollow(fd, &name)?;
        if info.st_mode & nix::libc::S_IFMT == nix::libc::S_IFDIR {
            let child = checked_child(fd, &name, &info)?;
            remove_contents(child.as_raw_fd())?;
            if !same(&info, &safefs::fstatat_nofollow(fd, &name)?) {
                return Err(io::Error::other(
                    "working directory entry replaced during removal",
                ));
            }
            safefs::rmdir_at(fd, &name)?;
        } else {
            safefs::unlink_at(fd, &name)?;
        }
    }
    Ok(())
}
