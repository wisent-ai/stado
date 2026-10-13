//! The register of waits open on this machine.
//!
//! A wait's lines go to the waiting program's stderr, which belongs to
//! whoever started it: a caller that filters that stream, or pipes it
//! through `tail`, sees nothing while a wait stands, and a wait that never
//! ended stayed invisible for hours. So each wait is also a file here,
//! created as it begins and removed as it ends, named by its id and holding
//! its `czekam` line. `stado waits` lists them, whoever started the process.
//!
//! The directory is `$STADO_WAITS_DIR` when set, else `$HOME/.stado/waits`.
//! A register that cannot be written does not stop the wait: the failure is
//! said on stderr beside the wait's own line, naming the file and the cause.

use std::path::PathBuf;

/// The variable that names the register directory in place of the home one.
pub const DIRECTORY_ENV: &str = "STADO_WAITS_DIR";

/// The register directory, or why this process has none.
pub fn directory() -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os(DIRECTORY_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    match std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        Some(home) => Ok(PathBuf::from(home).join(".stado").join("waits")),
        None => Err(format!(
            "neither {DIRECTORY_ENV} nor HOME is set, so the register of open waits has no directory"
        )),
    }
}

/// Record the wait `id` as open, holding its `czekam` line.
pub(crate) fn opened(id: &str, line: &str) {
    let written = directory().and_then(|directory| {
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("creating {}: {error}", directory.display()))?;
        let file = directory.join(id);
        std::fs::write(&file, format!("{line}\n"))
            .map_err(|error| format!("writing {}: {error}", file.display()))
    });
    if let Err(cause) = written {
        crate::line::write(&format!(
            "rejestr czekan niezapisany: id: {id}; przyczyna: {}",
            crate::line::field(&cause)
        ));
    }
}

/// Remove the wait `id` from the register: it ended.
pub(crate) fn closed(id: &str) {
    let removed = directory().and_then(|directory| {
        let file = directory.join(id);
        match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("removing {}: {error}", file.display())),
        }
    });
    if let Err(cause) = removed {
        crate::line::write(&format!(
            "rejestr czekan niezapisany: id: {id}; przyczyna: {}",
            crate::line::field(&cause)
        ));
    }
}

/// One wait the register holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    /// `<pid>-<n>`, the wait's id and its file's name.
    pub id: String,
    /// The process that began the wait.
    pub pid: u32,
    /// Whether that process still runs. A process that was killed before
    /// its wait ended left its file behind: the wait is not open, and its
    /// file says what it was waiting on when it died.
    pub alive: bool,
    /// The wait's `czekam` line: what, where, kind and since when.
    pub line: String,
}

/// Every wait the register holds, oldest id first. A register directory
/// that does not exist yet holds none; any other unreadable entry is the
/// error naming it.
pub fn listed() -> Result<Vec<Open>, String> {
    let directory = directory()?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("listing {}: {error}", directory.display())),
    };
    let mut waits = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("listing {}: {error}", directory.display()))?;
        let file = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        let pid = id
            .split_once('-')
            .and_then(|(pid, _)| pid.parse::<u32>().ok())
            .ok_or_else(|| format!("{} is not named <pid>-<n>", file.display()))?;
        let line = match std::fs::read_to_string(&file) {
            Ok(text) => text.trim_end().to_string(),
            // Its wait ended between the listing and the read.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("reading {}: {error}", file.display())),
        };
        waits.push(Open {
            id,
            pid,
            alive: running(pid),
            line,
        });
    }
    waits.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(waits)
}

/// Remove the file of a wait whose process no longer runs, and only such a
/// file: an open wait's file is its process's to remove.
pub fn forget(wait: &Open) -> Result<(), String> {
    if running(wait.pid) {
        return Err(format!(
            "process {} still runs, so wait {} is open and stays",
            wait.pid, wait.id
        ));
    }
    let file = directory()?.join(&wait.id);
    match std::fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("removing {}: {error}", file.display())),
    }
}

/// Whether a process with this pid exists: signal 0 delivers nothing and
/// answers whether it could be delivered (EPERM means it exists, owned by
/// another user).
fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: kill with signal 0 sends no signal; it only checks the pid.
    let answered = unsafe { libc::kill(pid, 0) };
    answered == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
