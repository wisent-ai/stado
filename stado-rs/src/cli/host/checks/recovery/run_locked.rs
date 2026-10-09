//! `stado host run-locked LOCK -- PROGRAM [ARGS…]`: run PROGRAM on this host
//! while holding an exclusive, non-blocking `flock(2)` on LOCK, the same lock
//! the storage-root transaction takes, so a repair and a transaction never
//! run together. PROGRAM inherits stdin, stdout and stderr, and its exit
//! status is this command's.

use std::fs::{DirBuilder, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::process::Command;

use crate::cli::CmdError;

const OWNER_ONLY_FILE: u32 = 0o600;
const OWNER_ONLY_DIRECTORY: u32 = 0o700;

pub fn run_locked(lock: &str, program: &[String]) -> Result<(), CmdError> {
    let (command, arguments) = program
        .split_first()
        .ok_or_else(|| CmdError::usage("run-locked needs a program after --"))?;
    let lock = Path::new(lock);
    if let Some(parent) = lock.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(OWNER_ONLY_DIRECTORY)
            .create(parent)
            .map_err(|error| {
                CmdError::click(format!("{}: {error}", parent.display()))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
    }
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(OWNER_ONLY_FILE)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(lock)
        .map_err(|error| {
            CmdError::click(format!("{}: {error}", lock.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    let locked =
        unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    if locked != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(nix::libc::EWOULDBLOCK) {
            return Err(CmdError::refused(
                "storage authority recovery is already running on this host",
            ));
        }
        return Err(CmdError::click(format!("{}: {error}", lock.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind())));
    }
    let status = crate::wait::status(Command::new(command).args(arguments)).map_err(|error| {
        CmdError::click(format!("{command} could not start: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    drop(file);
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => std::process::exit(code),
        None => Err(
            CmdError::click(format!("{command} ended by a signal: {status}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        ),
    }
}
