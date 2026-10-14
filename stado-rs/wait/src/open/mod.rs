//! The register of waits open on this machine.
//!
//! A wait's lines go to the waiting program's stderr, which belongs to
//! whoever started it: a caller that filters that stream, or pipes it
//! through `tail`, sees nothing while a wait stands, and a wait that never
//! ended stayed invisible for hours. So each wait is also a file here,
//! created as it begins and removed as it ends, named by its id and holding
//! its `czekam` line. `stado waits` lists them, whoever started the process.
//!
//! A process killed while it waited never removes its file. Reading the
//! register ([`listed`]) therefore ends every wait whose process no longer
//! runs: it reports that wait as ended, with the line it held when its
//! process died, and removes its file, so the register holds only waits a
//! running process stands in.
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

/// One wait the register held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    /// `<pid>-<n>`, the wait's id and its file's name.
    pub id: String,
    /// The process that began the wait.
    pub pid: u32,
    /// The wait's number within that process, counted from its start.
    pub sequence: u64,
    /// The wait's `czekam` line: what, where, kind and since when.
    pub line: String,
}

/// What one reading of the register found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Waits a running process stands in, oldest first by process and
    /// sequence.
    pub open: Vec<Open>,
    /// Waits whose process no longer runs, in the same order: each held the
    /// line it had when its process died, and its file is gone from the
    /// register as of this reading.
    pub ended: Vec<Open>,
}

/// Why the register could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterError {
    /// What failed, naming the directory or file and the cause.
    pub cause: String,
    /// The operating system's kind for a file operation that failed; none
    /// when this process has no register directory or a file in it is not
    /// named `<pid>-<n>`.
    pub io: Option<std::io::ErrorKind>,
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.cause)
    }
}

impl std::error::Error for RegisterError {}

fn io_failure(cause: String, error: &std::io::Error) -> RegisterError {
    RegisterError {
        cause: format!("{cause}: {error}"),
        io: Some(error.kind()),
    }
}

/// Read the register: every wait a running process stands in, and every
/// wait whose process no longer runs, which this reading ends by removing
/// its file. A register directory that does not exist yet holds none; an
/// entry that cannot be read or removed is the error naming it.
pub fn listed() -> Result<Listing, RegisterError> {
    let directory = directory().map_err(|cause| RegisterError { cause, io: None })?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Listing::default()),
        Err(error) => {
            return Err(io_failure(
                format!("listing {}", directory.display()),
                &error,
            ))
        }
    };
    let mut listing = Listing::default();
    for entry in entries {
        let entry = entry
            .map_err(|error| io_failure(format!("listing {}", directory.display()), &error))?;
        let file = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        let (pid, sequence) = id
            .split_once('-')
            .and_then(|(pid, sequence)| Some((pid.parse::<u32>().ok()?, sequence.parse().ok()?)))
            .ok_or_else(|| RegisterError {
                cause: format!(
                    "{} is not named <pid>-<n>, so it is no wait of this register; remove it",
                    file.display()
                ),
                io: None,
            })?;
        let line = match std::fs::read_to_string(&file) {
            Ok(text) => text.trim_end().to_string(),
            // Its wait ended between the listing and the read.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_failure(format!("reading {}", file.display()), &error)),
        };
        let wait = Open {
            id,
            pid,
            sequence,
            line,
        };
        if running(pid) {
            listing.open.push(wait);
            continue;
        }
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(io_failure(
                    format!(
                        "process {pid} no longer runs, so its wait {} ended, but removing {}",
                        wait.id,
                        file.display()
                    ),
                    &error,
                ))
            }
        }
        listing.ended.push(wait);
    }
    listing.open.sort_by_key(|wait| (wait.pid, wait.sequence));
    listing.ended.sort_by_key(|wait| (wait.pid, wait.sequence));
    Ok(listing)
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
