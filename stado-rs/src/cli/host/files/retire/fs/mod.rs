//! Descriptor-held filesystem primitives the retirement transaction is
//! built from. Every path component is opened with `O_NOFOLLOW` and
//! checked against the approved account uid.

pub(in crate::cli::host) mod dirs;
pub(in crate::cli::host) mod rename;

use std::ffi::{CString, OsStr};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::cli::CmdError;

use crate::cli::host::files::retire::retire_refused;

fn c_path_component(component: &OsStr, label: &str) -> Result<CString, CmdError> {
    CString::new(component.as_bytes())
        .map_err(|_| retire_refused(format!("{label} contains a NUL byte")))
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn require_owned_directory(metadata: &Metadata, uid: u32, label: &str) -> Result<(), CmdError> {
    if !metadata.is_dir() {
        return Err(retire_refused(format!("{label} is not a directory")));
    }
    if metadata.uid() != uid {
        return Err(retire_refused(format!(
            "{label} is not owned by the approved account"
        )));
    }
    if (metadata.mode() & 0o022).ne(&0) {
        return Err(retire_refused(format!(
            "{label} is group- or world-writable"
        )));
    }
    Ok(())
}

pub(super) fn open_home_directory(home: &Path, uid: u32) -> Result<File, CmdError> {
    let path_metadata = std::fs::symlink_metadata(home)
        .map_err(|error| retire_refused(format!("cannot inspect HOME: {error}")))?;
    if path_metadata.file_type().is_symlink() {
        return Err(retire_refused("HOME is a symlink"));
    }
    require_owned_directory(&path_metadata, uid, "HOME")?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(home)
        .map_err(|error| retire_refused(format!("cannot open HOME: {error}")))?;
    let opened = directory
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect opened HOME: {error}")))?;
    require_owned_directory(&opened, uid, "opened HOME")?;
    if !same_file(&path_metadata, &opened) {
        return Err(retire_refused("HOME changed while it was opened"));
    }
    Ok(directory)
}

pub(super) fn hash_open_file(file: &mut File) -> Result<String, CmdError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| retire_refused(format!("cannot seek file for hashing: {error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| retire_refused(format!("cannot hash file: {error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(super) fn source_unchanged(expected: &Metadata, observed: &Metadata) -> bool {
    same_file(expected, observed)
        && expected.uid() == observed.uid()
        && expected.mode() == observed.mode()
        && expected.len() == observed.len()
}
