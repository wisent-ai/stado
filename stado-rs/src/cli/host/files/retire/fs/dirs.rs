use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::{FromRawFd, RawFd};

use crate::cli::CmdError;

use crate::cli::host::files::retire::fs::{c_path_component, require_owned_directory};
use crate::cli::host::files::retire::retire_refused;

pub(in crate::cli::host) fn open_directory_at(
    parent: RawFd,
    name: &OsStr,
    uid: u32,
) -> Result<Option<File>, CmdError> {
    let name = c_path_component(name, "directory name")?;
    let fd = unsafe {
        nix::libc::openat(
            parent,
            name.as_ptr(),
            nix::libc::O_RDONLY
                | nix::libc::O_DIRECTORY
                | nix::libc::O_NOFOLLOW
                | nix::libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(nix::libc::ENOENT) {
            return Ok(None);
        }
        return Err(retire_refused(format!(
            "cannot open directory component {:?}: {error}",
            name
        )));
    }
    let directory = unsafe { File::from_raw_fd(fd) };
    let metadata = directory
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect directory: {error}")))?;
    require_owned_directory(&metadata, uid, "directory ancestor")?;
    Ok(Some(directory))
}

pub(in crate::cli::host) fn mkdir_at(parent: RawFd, name: &OsStr) -> Result<(), CmdError> {
    let name = c_path_component(name, "directory name")?;
    let result = unsafe { nix::libc::mkdirat(parent, name.as_ptr(), 0o700) };
    if result == 0 {
        Ok(())
    } else {
        Err(retire_refused(format!(
            "cannot create owner-only backup directory: {}",
            std::io::Error::last_os_error()
        )))
    }
}

pub(in crate::cli::host) fn open_or_create_directory_at(
    parent: RawFd,
    name: &OsStr,
    uid: u32,
    create: bool,
) -> Result<Option<File>, CmdError> {
    if let Some(directory) = open_directory_at(parent, name, uid)? {
        return Ok(Some(directory));
    }
    if !create {
        return Ok(None);
    }
    mkdir_at(parent, name)?;
    open_directory_at(parent, name, uid)?
        .map(Some)
        .ok_or_else(|| {
            retire_refused("backup directory disappeared immediately after it was created")
        })
}

pub(in crate::cli::host) fn open_source_at(
    parent: RawFd,
    name: &OsStr,
) -> Result<Option<File>, CmdError> {
    let name = c_path_component(name, "source basename")?;
    let fd = unsafe {
        nix::libc::openat(
            parent,
            name.as_ptr(),
            nix::libc::O_RDONLY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(nix::libc::ENOENT) {
            return Ok(None);
        }
        if error.raw_os_error() == Some(nix::libc::ELOOP) {
            return Err(retire_refused("source is a symlink"));
        }
        return Err(retire_refused(format!("cannot open source: {error}")));
    }
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}

pub(in crate::cli::host) fn entry_exists_at(parent: RawFd, name: &OsStr) -> Result<bool, CmdError> {
    let name = c_path_component(name, "path basename")?;
    let mut metadata = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
    let result = unsafe {
        nix::libc::fstatat(
            parent,
            name.as_ptr(),
            metadata.as_mut_ptr(),
            nix::libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(nix::libc::ENOENT) {
        Ok(false)
    } else {
        Err(retire_refused(format!(
            "cannot inspect directory entry: {error}"
        )))
    }
}

pub(in crate::cli::host) fn remove_empty_directory_at(parent: RawFd, name: &OsStr) {
    if let Ok(name) = c_path_component(name, "transaction name") {
        unsafe {
            nix::libc::unlinkat(parent, name.as_ptr(), nix::libc::AT_REMOVEDIR);
        }
    }
}
