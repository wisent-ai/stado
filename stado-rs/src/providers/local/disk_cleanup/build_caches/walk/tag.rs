//! The only thing that authorizes a deletion here: `CACHEDIR.TAG`, read
//! through the directory's own descriptor and believed only when it is a
//! regular file we own whose first line is the standard's signature.

use std::ffi::OsStr;
use std::io;
use std::os::fd::{AsRawFd, RawFd};

use nix::fcntl::OFlag;
use nix::sys::stat::Mode;

use crate::deploy::host_build_caches::CACHEDIR_SIGNATURE;
use crate::providers::local::disk_cleanup::build_caches::same_object;
use crate::providers::local::disk_cleanup::build_caches::walk::Walk;
use crate::providers::local::disk_cleanup::{euid, ifmt, safefs, JanitorError, IFREG};

/// The standard's file name; the signature lives in its first line.
const TAG_NAME: &str = "CACHEDIR.TAG";

/// Enough bytes to hold the signature line and prove where it ends. The
/// standard puts the signature at offset 0, so a longer read would only
/// widen what an attacker's file can say.
const TAG_READ_BYTES: usize = 64;

/// What `CACHEDIR.TAG` says about the directory holding it.
pub(super) enum Tag {
    /// Present, a regular file we own, first line is the signature.
    Signed,
    /// Present but not a valid tag: wrong first line, a symlink, a
    /// directory, or owned by someone else.
    Unsigned,
    /// No tag file at all — an ordinary directory on the way down.
    Absent,
}

impl<'a> Walk<'a> {
    /// Read `CACHEDIR.TAG` in the directory `dir_fd` names.
    pub(super) fn read_tag(&self, dir_fd: RawFd) -> Result<Tag, JanitorError> {
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
}
