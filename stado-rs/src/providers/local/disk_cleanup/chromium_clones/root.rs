//! Resolution of this account's per-user clone container, and of the clone
//! root inside it, from the platform rather than from the environment.

#[cfg(target_os = "macos")]
use std::ffi::OsString;
use std::path::PathBuf;

use super::names::{CLONE_CONTAINER, CLONE_ROOT_NAME};

/// This account's temporary container as macOS itself reports it
/// (`confstr(_CS_DARWIN_USER_TEMP_DIR)`, e.g.
/// `/var/folders/zy/l0_0w9dn0k94n1b7xnt7kpv80000gn/T/`), or `None` where the
/// platform has no such thing.
///
/// Read from libc and not from `$TMPDIR`, because the janitor runs both from a
/// launchd agent and from an ssh session, and only the first of those two is
/// guaranteed to carry that variable — a cleaner that silently found no root
/// over ssh would report a healthy no-op on the exact host whose disk was
/// full. The `unsafe` is the same shape as [`super::super::euid`]'s `geteuid`:
/// one libc call whose contract is a byte count into a buffer we own.
#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let mut buffer = vec![0u8; nix::libc::PATH_MAX as usize];
    let written = unsafe {
        nix::libc::confstr(
            nix::libc::_CS_DARWIN_USER_TEMP_DIR,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    // 0 is "this variable has no value"; a length past the buffer is a value
    // that was truncated, and a truncated path is not a path.
    if written == 0 || written > buffer.len() {
        return None;
    }
    buffer.truncate(written - 1); // confstr counts the terminating NUL
    let path = PathBuf::from(OsString::from_vec(buffer));
    path.is_absolute().then_some(path)
}

/// No per-user clone container exists off Apple platforms: macOS validates
/// code signatures this way and nothing else does.
#[cfg(not(target_os = "macos"))]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    None
}

/// Where the clones of this account's Chromium launches live, when the
/// platform has such a place.
pub fn default_root() -> Option<PathBuf> {
    let container = darwin_user_temp_dir()?.parent()?.to_path_buf();
    Some(container.join(CLONE_CONTAINER).join(CLONE_ROOT_NAME))
}
