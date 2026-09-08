use std::ffi::{CString, OsStr};
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;

use crate::cli::host::files::retire::fs::dirs::entry_exists_at;

#[cfg(target_os = "macos")]
pub(in crate::cli::host) fn rename_noreplace(
    source_parent: RawFd,
    source_name: &OsStr,
    destination_parent: RawFd,
    destination_name: &OsStr,
) -> std::io::Result<()> {
    let source_name = CString::new(source_name.as_bytes())?;
    let destination_name = CString::new(destination_name.as_bytes())?;
    let result = unsafe {
        nix::libc::renameatx_np(
            source_parent,
            source_name.as_ptr(),
            destination_parent,
            destination_name.as_ptr(),
            nix::libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
pub(in crate::cli::host) fn rename_noreplace(
    source_parent: RawFd,
    source_name: &OsStr,
    destination_parent: RawFd,
    destination_name: &OsStr,
) -> std::io::Result<()> {
    let source_name = CString::new(source_name.as_bytes())?;
    let destination_name = CString::new(destination_name.as_bytes())?;
    let result = unsafe {
        nix::libc::renameat2(
            source_parent,
            source_name.as_ptr(),
            destination_parent,
            destination_name.as_ptr(),
            nix::libc::RENAME_NOREPLACE as _,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(in crate::cli::host) fn rename_noreplace(
    _source_parent: RawFd,
    _source_name: &OsStr,
    _destination_parent: RawFd,
    _destination_name: &OsStr,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

pub(in crate::cli::host) fn rollback_retirement(
    source_parent: RawFd,
    source_name: &OsStr,
    destination_parent: RawFd,
    destination_name: &OsStr,
) -> String {
    match entry_exists_at(source_parent, source_name) {
        Ok(true) => {
            "source path is still present; archived entry was retained for inspection".to_string()
        }
        Ok(false) => match rename_noreplace(
            destination_parent,
            destination_name,
            source_parent,
            source_name,
        ) {
            Ok(()) => "source was restored by atomic no-replace rename".to_string(),
            Err(error) => {
                format!("source restoration failed after the postcondition mismatch: {error}")
            }
        },
        Err(error) => format!("source restoration could not inspect the source path: {error}"),
    }
}
