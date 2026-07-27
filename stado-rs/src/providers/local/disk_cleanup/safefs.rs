//! Dir-fd-relative filesystem primitives for the disk cleaner.
//!
//! These are the Rust spellings of the `os.open(..., dir_fd=...)` /
//! `os.unlink(..., dir_fd=...)` calls the Python janitor
//! (`stado/providers/local/disk/cleanup.py`) builds its sandbox on. Every
//! operation takes an already-open, already-validated directory descriptor
//! plus a single path component, so a swapped or symlinked ancestor can
//! never redirect the operation outside the validated tree.
//!
//! `unsafe` in the disk-cleanup port is confined to this file, in two
//! reviewed helpers: [`rename_exchange`] (the `renameat2`/`renameatx_np`
//! syscall wrapper, mirroring Python's `ctypes` call in `_hf_exchange`)
//! and [`borrowed`] (a `BorrowedFd::borrow_raw` adapter for the nix
//! wrappers, valid because every descriptor passed in outlives the call).

use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use nix::fcntl::OFlag;
use nix::sys::stat::{FileStat, Mode};

/// Python `getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_DIRECTORY", 0)`
/// for read-only directory opens.
fn dir_flags() -> OFlag {
    OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW
}

/// Map a nix errno to `std::io::Error` (nix and io use the same raw codes).
fn io_err(errno: nix::Error) -> io::Error {
    io::Error::from_raw_os_error(errno as i32)
}

/// Adapt a raw descriptor for nix's `AsFd` parameters.
///
/// SAFETY contract: every caller passes a descriptor that stays open for
/// the whole call (the janitor owns all of them as `OwnedFd`/`File`).
fn borrowed(fd: RawFd) -> BorrowedFd<'static> {
    // SAFETY: upheld by the module contract above — no descriptor is
    // closed while a nix call borrowing it is in flight. The 'static is
    // fictional but never escapes the individual nix call.
    unsafe { BorrowedFd::borrow_raw(fd) }
}

/// Python `os.open(name, O_RDONLY|O_DIRECTORY|O_NOFOLLOW, dir_fd=parent)`.
pub fn open_dir_at(parent: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
    nix::fcntl::openat(borrowed(parent), name, dir_flags(), Mode::empty()).map_err(io_err)
}

/// Python `os.open(path, O_RDONLY|O_DIRECTORY|O_NOFOLLOW)` on an
/// already-resolved absolute path (the validated cache root, the state
/// dir for its fsync).
pub fn open_dir_path(path: &std::path::Path) -> io::Result<OwnedFd> {
    nix::fcntl::open(path, dir_flags(), Mode::empty()).map_err(io_err)
}

/// Python `os.open(name, flags, dir_fd=parent)` for regular files
/// (`O_NOFOLLOW` always set by the callers here, as in the Python source).
pub fn open_file_at(parent: RawFd, name: &OsStr, flags: OFlag, mode: Mode) -> io::Result<OwnedFd> {
    nix::fcntl::openat(borrowed(parent), name, flags | OFlag::O_NOFOLLOW, mode).map_err(io_err)
}

/// Python `os.dup(fd)`.
pub fn dup_fd(fd: RawFd) -> io::Result<OwnedFd> {
    nix::unistd::dup(borrowed(fd)).map_err(io_err)
}

/// Python `_hf_open_path`: descend `parts` beneath `root_fd`, opening each
/// component with `O_DIRECTORY|O_NOFOLLOW` and closing the parent behind us.
pub fn open_path(root_fd: RawFd, parts: &[OsString]) -> io::Result<OwnedFd> {
    let mut descriptor = dup_fd(root_fd)?;
    for part in parts {
        let child = open_dir_at(descriptor.as_raw_fd(), part);
        drop(descriptor);
        match child {
            Ok(child) => descriptor = child,
            Err(exc) => return Err(exc),
        }
    }
    Ok(descriptor)
}

/// Python `os.stat(name, dir_fd=parent, follow_symlinks=False)`.
pub fn fstatat_nofollow(parent: RawFd, name: &OsStr) -> io::Result<FileStat> {
    nix::sys::stat::fstatat(
        borrowed(parent),
        name,
        nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
    )
    .map_err(io_err)
}

/// Python `os.fstat(fd)`.
pub fn fstat(fd: RawFd) -> io::Result<FileStat> {
    nix::sys::stat::fstat(borrowed(fd)).map_err(io_err)
}

/// Python `os.mkdir(name, mode, dir_fd=parent)`.
pub fn mkdir_at(parent: RawFd, name: &OsStr, mode: Mode) -> io::Result<()> {
    nix::sys::stat::mkdirat(borrowed(parent), name, mode).map_err(io_err)
}

/// Python `os.unlink(name, dir_fd=parent)`.
pub fn unlink_at(parent: RawFd, name: &OsStr) -> io::Result<()> {
    nix::unistd::unlinkat(
        borrowed(parent),
        name,
        nix::unistd::UnlinkatFlags::NoRemoveDir,
    )
    .map_err(io_err)
}

/// Python `os.rmdir(name, dir_fd=parent)`.
pub fn rmdir_at(parent: RawFd, name: &OsStr) -> io::Result<()> {
    nix::unistd::unlinkat(
        borrowed(parent),
        name,
        nix::unistd::UnlinkatFlags::RemoveDir,
    )
    .map_err(io_err)
}

/// Python `os.link(name, name, src_dir_fd=..., dst_dir_fd=..., follow_symlinks=False)`
/// (linkat never follows without AT_SYMLINK_FOLLOW).
pub fn link_at(src_parent: RawFd, dst_parent: RawFd, name: &OsStr) -> io::Result<()> {
    nix::unistd::linkat(
        borrowed(src_parent),
        name,
        borrowed(dst_parent),
        name,
        nix::fcntl::AtFlags::empty(),
    )
    .map_err(io_err)
}

/// Python `os.readlink(name, dir_fd=parent)`.
pub fn readlink_at(parent: RawFd, name: &OsStr) -> io::Result<OsString> {
    nix::fcntl::readlinkat(borrowed(parent), name).map_err(io_err)
}

/// Python `os.fchmod(fd, mode)`.
pub fn fchmod(fd: RawFd, mode: Mode) -> io::Result<()> {
    nix::sys::stat::fchmod(borrowed(fd), mode).map_err(io_err)
}

/// Python `os.fsync(fd)`.
pub fn fsync(fd: RawFd) -> io::Result<()> {
    nix::unistd::fsync(borrowed(fd)).map_err(io_err)
}

/// Python `os.read(fd, n)` (single read of at most `n` bytes).
pub fn read_fd(fd: RawFd, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    let got = nix::unistd::read(borrowed(fd), &mut buf).map_err(io_err)?;
    buf.truncate(got);
    Ok(buf)
}

/// One `os.scandir(dir_fd)` entry name. Types are never trusted from
/// `d_type` — like the Python, every decision re-stats with
/// `AT_SYMLINK_NOFOLLOW`.
pub struct DirEntries {
    inner: nix::dir::OwningIter,
}

impl DirEntries {
    /// Python `os.scandir(directory_fd)` (which duplicates the descriptor):
    /// open a FRESH descriptor for `.` beneath `directory_fd` so the scan
    /// has its own open file description and cannot disturb, or be
    /// disturbed by, other users of `directory_fd`.
    pub fn open(directory_fd: RawFd) -> io::Result<DirEntries> {
        let fd = nix::fcntl::openat(
            borrowed(directory_fd),
            ".",
            OFlag::O_RDONLY | OFlag::O_DIRECTORY,
            Mode::empty(),
        )
        .map_err(io_err)?;
        let dir = nix::dir::Dir::from_fd(fd).map_err(io_err)?;
        Ok(DirEntries {
            inner: dir.into_iter(),
        })
    }
}

impl Iterator for DirEntries {
    type Item = io::Result<OsString>;

    fn next(&mut self) -> Option<Self::Item> {
        // nix's readdir wrapper yields "." and ".." (os.scandir does not).
        for entry in self.inner.by_ref() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(exc) => return Some(Err(io_err(exc))),
            };
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            return Some(Ok(OsStr::from_bytes(name).to_os_string()));
        }
        None
    }
}

/// `RENAME_EXCHANGE` on both supported platforms (Python passes the
/// literal `0x00000002`).
const RENAME_EXCHANGE_FLAGS: u32 = 0x0000_0002;

/// Atomically exchange two names beneath the already-validated cache root.
/// Python `_hf_exchange`: `renameatx_np` on Darwin, `renameat2` elsewhere
/// (via `ctypes` on the process libc).
///
/// SAFETY: both name pointers come from live `CString`s that outlive the
/// call; `dirfd` is an open directory descriptor owned by the caller; the
/// flags constant matches the Python literal; the kernel writes no
/// userspace memory here. On Linux the raw `renameat2` syscall is used
/// (glibc wrappers are not universal); an `ENOSYS` kernel is reported as
/// `ENOTSUP`, matching the Python `getattr` fallback. On macOS
/// `renameatx_np` is a stable libc symbol.
#[cfg(target_os = "macos")]
pub fn rename_exchange(dirfd: RawFd, first: &OsStr, second: &OsStr) -> io::Result<()> {
    let first_bytes = CString::new(first.as_bytes()).map_err(|_| einval("nul in name"))?;
    let second_bytes = CString::new(second.as_bytes()).map_err(|_| einval("nul in name"))?;
    let result = unsafe {
        nix::libc::renameatx_np(
            dirfd,
            first_bytes.as_ptr(),
            dirfd,
            second_bytes.as_ptr(),
            RENAME_EXCHANGE_FLAGS,
        )
    };
    if result != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Linux spelling of [`rename_exchange`] (see its docs for the SAFETY
/// contract).
#[cfg(target_os = "linux")]
pub fn rename_exchange(dirfd: RawFd, first: &OsStr, second: &OsStr) -> io::Result<()> {
    let first_bytes = CString::new(first.as_bytes()).map_err(|_| einval("nul in name"))?;
    let second_bytes = CString::new(second.as_bytes()).map_err(|_| einval("nul in name"))?;
    let result = unsafe {
        nix::libc::syscall(
            nix::libc::SYS_renameat2,
            dirfd,
            first_bytes.as_ptr(),
            dirfd,
            second_bytes.as_ptr(),
            RENAME_EXCHANGE_FLAGS,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let exc = io::Error::last_os_error();
    if exc.raw_os_error() == Some(nix::libc::ENOSYS) {
        return Err(io::Error::from_raw_os_error(nix::libc::ENOTSUP));
    }
    Err(exc)
}

/// Unsupported platforms fail closed, exactly like Python's
/// `OSError(ENOTSUP, "atomic directory exchange unavailable")`.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn rename_exchange(_dirfd: RawFd, _first: &OsStr, _second: &OsStr) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(nix::libc::ENOTSUP))
}

fn einval(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}
