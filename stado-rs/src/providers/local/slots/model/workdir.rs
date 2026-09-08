//! The queue-owned job tree a slot executes in: locating it, opening a file
//! inside it that only this agent may have created, and the one diagnostic a
//! vanished or replaced tree is reported with.

use super::*;

pub(crate) fn job_work_dir(job_id: &str) -> std::io::Result<PathBuf> {
    super::disk_cleanup::queue_workdirs::work_dir(job_id)
}

pub(crate) fn open_agent_reserved_file(path: &Path) -> io::Result<File> {
    match std::fs::symlink_metadata(path) {
        Ok(info)
            if !info.file_type().is_file()
                || info.file_type().is_symlink()
                || info.uid() != super::disk_cleanup::euid() =>
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("unsafe agent-reserved file {}", path.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .mode(0o600)
        .open(path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.uid() != super::disk_cleanup::euid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("unsafe agent-reserved file {}", path.display()),
        ));
    }
    // O_NONBLOCK makes a FIFO/device replacement fail admission instead of
    // hanging this agent before fstat. A verified regular file does not need
    // the flag, so clear it before the descriptor is inherited by the child.
    let fd = file.as_raw_fd();
    // SAFETY: fd belongs to the live `file`; F_GETFL/F_SETFL do not dereference
    // userspace pointers, and failure is returned before the file is used.
    let flags = unsafe { nix::libc::fcntl(fd, nix::libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & nix::libc::O_NONBLOCK != 0 {
        let result =
            unsafe { nix::libc::fcntl(fd, nix::libc::F_SETFL, flags & !nix::libc::O_NONBLOCK) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

pub(crate) fn work_dir_is_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|info| info.is_dir() && !info.file_type().is_symlink())
}

pub(crate) fn workdir_missing_diagnostic(job_id: &str, phase: &str, path: &Path) -> String {
    format!(
        "workdir_missing job_id={job_id} phase={phase} expected_path={}",
        path.display()
    )
}
