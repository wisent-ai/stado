//! The only thing that authorizes a deletion here: `CACHEDIR.TAG`, read
//! through the directory's own descriptor and believed only when it is a
//! regular file we own whose first line is the standard's signature.

use std::ffi::OsStr;
use std::io;
use std::os::fd::{AsRawFd, RawFd};

use nix::fcntl::OFlag;
use nix::sys::stat::Mode;

use crate::deploy::host_build_caches::CACHEDIR_SIGNATURE;
use crate::providers::local::space_reclaim::build_caches::same_object;
use crate::providers::local::space_reclaim::build_caches::walk::Walk;
use crate::providers::local::space_reclaim::{euid, ifmt, safefs, JanitorError, IFREG};

/// The standard's file name; the file must begin with the signature
/// (https://bford.info/cachedir/).
const TAG_NAME: &str = "CACHEDIR.TAG";

/// What `CACHEDIR.TAG` says about the directory holding it.
pub(super) enum Tag {
    /// Present, a regular file we own, beginning with the signature.
    Signed,
    /// Present but not a valid tag: it does not begin with the signature,
    /// or it is a symlink, a directory, or owned by someone else.
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
        // Exactly the signature's length is read: the standard puts it at
        // offset 0, so a longer read would only widen what a file can say.
        let payload = safefs::read_fd(descriptor.as_raw_fd(), CACHEDIR_SIGNATURE.len())?;
        if payload == CACHEDIR_SIGNATURE.as_bytes() {
            Ok(Tag::Signed)
        } else {
            Ok(Tag::Unsigned)
        }
    }
}
