//! The register of waits open on this machine.
//!
//! A wait's lines go to the waiting program's stderr, which belongs to
//! whoever started it: a caller that filters that stream, or pipes it
//! through `tail`, sees nothing while a wait stands, and a wait that never
//! ends would stay invisible. So each wait is also a file here,
//! created as it begins and removed as it ends, named by its id and holding
//! its `czekam` line. `stado waits` lists them, whoever started the process.
//!
//! A process killed while it waited never removes its file. Reading the
//! register ([`listed`]) therefore ends every wait whose process no longer
//! runs: it reports that wait as ended, with the line it held when its
//! process died, and removes its file, so the register holds only waits a
//! running process stands in. A pid is read against the birth of the
//! process holding it now ([`birth_seconds`]): a process born after the
//! wait's file was written is another process that was handed the pid, so
//! the wait's own process has ended.
//!
//! The directory is `$STADO_WAITS_DIR` when set, else `$HOME/.stado/waits`.
//! A register that cannot be written does not stop the wait: the failure is
//! said on stderr beside the wait's own line, naming the file and the cause.

use std::path::PathBuf;
use std::time::SystemTime;

mod birth;
pub use birth::birth_seconds;

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
        let misnamed = |why: String| RegisterError {
            cause: format!(
                "{} is not named <pid>-<n> ({why}), so it is no wait of this register; remove it",
                file.display()
            ),
            io: None,
        };
        let Some((pid, sequence)) = id.split_once('-') else {
            return Err(misnamed("no dash".to_string()));
        };
        let pid = pid
            .parse::<u32>()
            .map_err(|error| misnamed(format!("pid {pid:?}: {error}")))?;
        let sequence = sequence
            .parse::<u64>()
            .map_err(|error| misnamed(format!("sequence {sequence:?}: {error}")))?;
        let line = match std::fs::read_to_string(&file) {
            Ok(text) => text.trim_end().to_string(),
            // Its wait ended between the listing and the read.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_failure(format!("reading {}", file.display()), &error)),
        };
        let written = match std::fs::metadata(&file).and_then(|metadata| metadata.modified()) {
            Ok(written) => written,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(io_failure(
                    format!("reading when {} was written", file.display()),
                    &error,
                ))
            }
        };
        let wait = Open {
            id,
            pid,
            sequence,
            line,
        };
        let stands = writer_stands(pid, written).map_err(|cause| RegisterError {
            cause: format!(
                "whether the process of wait {} still runs: {cause}",
                wait.id
            ),
            io: None,
        })?;
        if stands {
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

/// Whether the process that wrote a record at `written` under `pid` still
/// runs: a process holds the pid and was born no later than the record. One
/// born after it was handed the pid when the writer ended, so the writer no
/// longer runs, whoever owns the pid now.
#[cfg(unix)]
pub(crate) fn writer_stands(pid: u32, written: SystemTime) -> Result<bool, String> {
    if !running(pid) {
        return Ok(false);
    }
    let process = libc::pid_t::try_from(pid).map_err(|error| format!("pid {pid}: {error}"))?;
    let Some(born) = birth_seconds(process)? else {
        return Ok(false);
    };
    let written = written
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("the record of pid {pid} is dated before the epoch: {error}"))?
        .as_secs();
    Ok(born <= written)
}

/// Whether a writer still runs is read from kill(2) and the process's birth
/// time, which only a unix system answers; elsewhere the question is refused
/// naming the pid, as `birth_seconds` refuses its own.
#[cfg(not(unix))]
pub(crate) fn writer_stands(pid: u32, _written: SystemTime) -> Result<bool, String> {
    Err(format!(
        "whether pid {pid} still runs is unsupported on this operating system"
    ))
}

/// kill(2)'s null signal, which delivers nothing and only checks the pid:
/// https://pubs.opengroup.org/onlinepubs/9699919799/functions/kill.html
#[cfg(unix)]
const NULL_SIGNAL: libc::c_int = 0;

/// Whether a process with this pid exists: the null signal answers whether
/// it could be delivered (EPERM means it exists, owned by another user).
#[cfg(unix)]
pub(crate) fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: kill with the null signal sends no signal; it only checks the pid.
    let answered = unsafe { libc::kill(pid, NULL_SIGNAL) };
    !answered.is_negative() || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
