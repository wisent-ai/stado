//! The owned root components, and admission of one job's canonical tree.
//!
//! Every path this cleaner may touch is built here. Each component below the
//! physically resolved home is opened separately with
//! `O_DIRECTORY|O_NOFOLLOW` and re-validated as an owned directory, so no
//! component symlink and no foreign owner can redirect either admission or
//! cleanup.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

use nix::libc::dev_t;
use nix::sys::stat::{FileStat, Mode};

use crate::providers::local::disk_cleanup::queue_workdirs::WORKDIR_PREFIX;
use crate::providers::local::disk_cleanup::{euid, ifmt, safefs, IFDIR};

/// Queue-owned workdir root, relative to the account that runs the agent.
pub const WORK_ROOT: &str = ".stado/work/jobs";

/// Queue root components below the already-resolved agent home. Each is opened
/// separately with `O_DIRECTORY|O_NOFOLLOW`; no component symlink is supported.
const WORK_ROOT_COMPONENTS: [&str; 3] = [".stado", "work", "jobs"];

/// Canonical owner-visible root beneath an already-resolved agent home.
pub fn work_root_in(home: &Path) -> PathBuf {
    home.join(WORK_ROOT)
}

fn resolved_home(home: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(home)
}

/// Canonical owner-visible root for every local queue job.
pub fn work_root() -> PathBuf {
    let home = crate::config_file::expand_tilde("~");
    work_root_in(&resolved_home(&home).unwrap_or(home))
}

fn validate_owned_directory(fd: RawFd, label: &Path) -> io::Result<FileStat> {
    let info = safefs::fstat(fd)?;
    if ifmt(info.st_mode as u32) != IFDIR || info.st_uid != euid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("unsafe queue directory {}", label.display()),
        ));
    }
    Ok(info)
}

fn open_owned_component(
    parent: RawFd,
    name: &OsStr,
    label: &Path,
    create: bool,
) -> io::Result<OwnedFd> {
    let opened = match safefs::open_dir_at(parent, name) {
        Ok(fd) => fd,
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
            match safefs::mkdir_at(parent, name, Mode::from_bits_truncate(0o700)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            safefs::open_dir_at(parent, name)?
        }
        Err(error) => return Err(error),
    };
    validate_owned_directory(opened.as_raw_fd(), label)?;
    safefs::fchmod(opened.as_raw_fd(), Mode::from_bits_truncate(0o700))?;
    Ok(opened)
}

pub(super) fn open_work_root_in(
    home: &Path,
    create: bool,
) -> io::Result<(PathBuf, OwnedFd, dev_t)> {
    let home = resolved_home(home)?;
    let home_fd = safefs::open_dir_path(&home)?;
    let home_info = validate_owned_directory(home_fd.as_raw_fd(), &home)?;
    let mut parent = home_fd;
    let mut path = home.clone();
    for component in WORK_ROOT_COMPONENTS {
        path.push(component);
        parent = open_owned_component(parent.as_raw_fd(), OsStr::new(component), &path, create)?;
    }
    Ok((path, parent, home_info.st_dev))
}

/// Canonical workdir for one validated local queue job.
pub fn work_dir(job_id: &str) -> io::Result<PathBuf> {
    if !valid_job_id(job_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "queue job id must be job- followed by 24 lowercase hex digits",
        ));
    }
    Ok(work_root().join(format!("{WORKDIR_PREFIX}{job_id}")))
}

fn valid_job_id(id: &str) -> bool {
    crate::queue::submit::is_canonical_job_id(id)
}

/// Create one canonical job tree without accepting a symlink or foreign owner.
pub fn create_work_dir(job_id: &str) -> io::Result<PathBuf> {
    if !valid_job_id(job_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "queue job id must be job- followed by 24 lowercase hex digits",
        ));
    }
    let (root, root_fd, _) = open_work_root_in(&crate::config_file::expand_tilde("~"), true)?;
    let name = OsString::from(format!("{WORKDIR_PREFIX}{job_id}"));
    let work = root.join(&name);
    let work_fd = open_owned_component(root_fd.as_raw_fd(), &name, &work, true)?;
    let output = work.join("output");
    open_owned_component(work_fd.as_raw_fd(), OsStr::new("output"), &output, true)?;
    Ok(work)
}

/// The safe job id encoded by a canonical workdir name.
pub(super) fn job_id(name: &str) -> Option<&str> {
    let id = name.strip_prefix(WORKDIR_PREFIX)?;
    valid_job_id(id).then_some(id)
}
