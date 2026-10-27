//! What the kernel says about one process: whether a pid still lives, how it
//! is asked to stop, and whether it runs the executable it should.
//!
//! Split out of `serving/discover.rs`, which had grown past the module line
//! cap; recognising the release proxy among those processes stays there.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

use crate::release_agent::state::records::ProcessRecord;

/// init, the first process the kernel starts: https://man7.org/linux/man-pages/man1/init.1.html
const INIT_PID: i32 = 1;

/// The argument the release proxy's command line carries after its program.
const SERVE_ARGUMENT: &str = " serve";

/// What Linux appends to /proc/<pid>/exe when the image was replaced on disk: https://man7.org/linux/man-pages/man5/proc_pid_exe.5.html
const DELETED_SUFFIX: &str = " (deleted)";

/// Whether a release process still lives, as `primitives::procutil` answers
/// for every pid; init is never a release process this owner runs, so it is
/// not one here.
pub(crate) fn pid_alive(pid: i32) -> bool {
    if pid <= INIT_PID {
        return false;
    }
    crate::primitives::procutil::pid_alive(pid)
}

/// SIGTERM to a release process's group, else to the process itself. A process
/// neither signal reached keeps serving the release its caller is retiring, so
/// that is said with the pid and cause.
pub(crate) fn terminate(record: &ProcessRecord) {
    if pid_alive(record.pid) {
        let group = Pid::from_raw(-record.pid);
        if kill(group, Signal::SIGTERM).is_err() {
            match kill(Pid::from_raw(record.pid), Signal::SIGTERM) {
                Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                Err(errno) => eprintln!(
                    "stado: SIGTERM to release process {} failed: {errno}; it is still running",
                    record.pid
                ),
            }
        }
    }
}

/// Two paths that name one executable, whatever spelling each was reached by.
///
/// `~/.local/bin/stado` is a symlink to `~/.stado/bin/stado` on every Mac in
/// this fleet, so a process started through the first and a check that
/// resolved the second are the same program under two names. String equality
/// called them different, and the credential plane refused every read with
/// `recorded stable proxy pid N does not match the exact executable and
/// arguments` while the proxy it was refusing was its own.
pub(crate) fn same_executable(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Does `pid` run the executable at `expected`?
///
/// The two systems answer this in different words. Linux hands the resolved
/// image through `/proc/<pid>/exe`. macOS answers `ps -o comm=`, and on this
/// fleet that prints the bare program name — `stado`, not a path — so the
/// former comparison against a full path was false for every process on
/// every Mac. The full path is still proved, by the complete argument vector
/// the callers compare; this check is the image, and a bare name is compared
/// as one.
///
/// An image that cannot be read is the error, not a mismatch: answered as
/// "not that executable", a control peer whose image this host could not
/// inspect was refused as an impostor.
pub(crate) fn process_executable_matches(pid: i32, expected: &Path) -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        let actual = match std::fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(actual) => actual,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("cannot read the image of pid {pid}: {error}")),
        };
        let actual = actual.to_string_lossy();
        // A replaced image still names the path it was started from, before the suffix.
        let actual = match actual.strip_suffix(DELETED_SUFFIX) {
            Some(started_from) => started_from,
            None => &actual,
        };
        Ok(same_executable(Path::new(actual), expected))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = crate::wait::output(Command::new("/bin/ps").args([
            "-p",
            &pid.to_string(),
            "-o",
            "comm=",
        ]))
        .map_err(|error| format!("cannot read the image of pid {pid}: {error}"))?;
        let actual = String::from_utf8_lossy(&output.stdout);
        let actual = Path::new(actual.trim());
        // `ps -p` exits non-zero with no row for a pid that is gone.
        if !output.status.success() || actual.as_os_str().is_empty() {
            return Ok(false);
        }
        if actual.is_absolute() {
            return Ok(same_executable(actual, expected));
        }
        Ok(actual.file_name() == expected.file_name())
    }
}

/// The control peer is the actual host executable, not merely a process that
/// supplied its PID in JSON. Managed and global installations can be separate
/// byte-identical files; the ordinary same-file predicate remains unchanged.
pub(crate) fn controller_process_matches(pid: i32) -> Result<bool, String> {
    if !pid_alive(pid) {
        return Ok(false);
    }
    let expected = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    let output = crate::wait::output(Command::new("/bin/ps").args([
        "-ww",
        "-p",
        &pid.to_string(),
        "-o",
        "command=",
    ]))
    .map_err(|error| format!("cannot inspect proxy owner pid {pid}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect proxy owner pid {pid}: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let observed = String::from_utf8_lossy(&output.stdout);
    let command = observed.trim();
    for (index, _) in command.match_indices(SERVE_ARGUMENT) {
        let remaining = &command[index + SERVE_ARGUMENT.len()..];
        if !remaining.is_empty() && !remaining.starts_with(char::is_whitespace) {
            continue;
        }
        let program = Path::new(&command[..index]);
        if process_executable_matches(pid, program)?
            && (same_executable(program, &expected)
                || same_native_image(program, &expected).map_err(|error| {
                    format!(
                        "cannot compare proxy owner image {} with {}: {error}",
                        program.display(),
                        expected.display()
                    )
                })?)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether two files hold the same bytes, compared through buffered reads as
/// they come; files of different lengths are not compared at all.
fn same_native_image(left: &Path, right: &Path) -> std::io::Result<bool> {
    let left_file = std::fs::File::open(left)?;
    let right_file = std::fs::File::open(right)?;
    let left_metadata = left_file.metadata()?;
    let right_metadata = right_file.metadata()?;
    if !left_metadata.is_file()
        || !right_metadata.is_file()
        || left_metadata.len() != right_metadata.len()
    {
        return Ok(false);
    }
    let mut left = BufReader::new(left_file);
    let mut right = BufReader::new(right_file);
    loop {
        let (count, same, left_ended, right_ended) = {
            let left_bytes = left.fill_buf()?;
            let right_bytes = right.fill_buf()?;
            let count = left_bytes.len().min(right_bytes.len());
            (count, left_bytes[..count] == right_bytes[..count], left_bytes.is_empty(), right_bytes.is_empty())
        };
        if left_ended || right_ended {
            return Ok(left_ended && right_ended);
        }
        if !same {
            return Ok(false);
        }
        left.consume(count);
        right.consume(count);
    }
}
