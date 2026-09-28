//! The two Darwin primitives the transaction is built on: `clonefile(2)`
//! copy-on-write clones and BSD file flags. Other systems have neither; the
//! transaction refuses there before any checkpoint is taken.

use std::fs::Metadata;
use std::io;

#[cfg(target_os = "macos")]
mod native {
    use std::ffi::CString;
    use std::fs::Metadata;
    use std::io;
    use std::os::macos::fs::MetadataExt;

    pub const SUPPORTED: bool = true;
    pub const USER_IMMUTABLE: u32 = nix::libc::UF_IMMUTABLE;
    pub const ANY_IMMUTABLE: u32 = nix::libc::UF_IMMUTABLE | nix::libc::SF_IMMUTABLE;

    fn c_path(path: &str) -> io::Result<CString> {
        CString::new(path).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }

    pub fn flags(info: &Metadata) -> u32 {
        info.st_flags()
    }

    pub fn set_flags(path: &str, flags: u32) -> io::Result<()> {
        let path = c_path(path)?;
        // SAFETY: `path` is a NUL-terminated string that outlives the call.
        if unsafe { nix::libc::chflags(path.as_ptr(), flags) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn clone(source: &str, destination: &str) -> io::Result<()> {
        let source = c_path(source)?;
        let destination = c_path(destination)?;
        // SAFETY: both paths are NUL-terminated strings that outlive the call.
        if unsafe { nix::libc::clonefile(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use std::fs::Metadata;
    use std::io;

    pub const SUPPORTED: bool = false;
    pub const USER_IMMUTABLE: u32 = 0;
    pub const ANY_IMMUTABLE: u32 = 0;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "copy-on-write clones and file flags exist only on Darwin",
        )
    }

    pub fn flags(_info: &Metadata) -> u32 {
        0
    }

    pub fn set_flags(_path: &str, _flags: u32) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn clone(_source: &str, _destination: &str) -> io::Result<()> {
        Err(unsupported())
    }
}

/// Whether this system has `clonefile(2)` and BSD file flags.
pub(super) const SUPPORTED: bool = native::SUPPORTED;
/// The owner-settable immutable flag the checkpoints are sealed with.
pub(super) const USER_IMMUTABLE: u32 = native::USER_IMMUTABLE;
/// Either immutable flag; a staged clone carrying one came from `sudo cp`.
pub(super) const ANY_IMMUTABLE: u32 = native::ANY_IMMUTABLE;

pub(super) fn flags(info: &Metadata) -> u32 {
    native::flags(info)
}

pub(super) fn set_flags(path: &str, flags: u32) -> io::Result<()> {
    native::set_flags(path, flags)
}

pub(super) fn clone(source: &str, destination: &str) -> io::Result<()> {
    native::clone(source, destination)
}
