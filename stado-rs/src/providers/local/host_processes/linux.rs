//! Process counters on Linux, from procfs.
//!
//! CPU comes from `/proc/<pid>/schedstat`, whose first field is the time the
//! process has run on a CPU in nanoseconds; the program from `comm`;
//! resident memory from `statm`'s resident pages; storage bytes from `io`,
//! which the kernel lets only the process's own account read. Linux keeps
//! reaped children's CPU only among `stat`'s positional fields, which this
//! reader does not take, so the report carries no children figure here, and
//! no start record, so the program and non-decreasing counters stand for a
//! process's identity.

use std::io::ErrorKind;
use std::path::Path;

use nix::unistd::{sysconf, SysconfVar};

use super::{Counters, DiskBytes, Reading};

fn page_size() -> Result<u64, String> {
    let size = sysconf(SysconfVar::PAGE_SIZE)
        .map_err(|error| format!("sysconf(PAGE_SIZE) failed: {error}"))?
        .ok_or_else(|| "sysconf(PAGE_SIZE) reports no page size".to_string())?;
    u64::try_from(size).map_err(|error| format!("sysconf(PAGE_SIZE) gave {size}: {error}"))
}

fn number(text: &str, what: &str) -> Result<u64, String> {
    text.trim()
        .parse()
        .map_err(|error| format!("{what} reads {text:?}, not a count: {error}"))
}

/// The value of `key:` in a `key: value` procfs file.
fn field(text: &str, key: &str, file: &Path) -> Result<u64, String> {
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .ok_or_else(|| format!("{} has no {key} line", file.display()))?;
    number(line, &format!("{} {key}", file.display()))
}

/// Why one process could not be read: it ended, or the kernel refused.
enum Unread {
    Ended,
    Refused(String),
}

fn read_file(path: &Path) -> Result<String, Unread> {
    std::fs::read_to_string(path).map_err(|error| match error.kind() {
        ErrorKind::NotFound => Unread::Ended,
        _ => Unread::Refused(format!("{}: {error}", path.display())),
    })
}

fn counters(process: &Path, page: u64) -> Result<Counters, Unread> {
    let schedstat = process.join("schedstat");
    let ran = read_file(&schedstat)?;
    let cpu_ns = ran
        .split_whitespace()
        .next()
        .ok_or_else(|| Unread::Refused(format!("{} is empty", schedstat.display())))
        .and_then(|first| {
            number(first, &schedstat.display().to_string()).map_err(Unread::Refused)
        })?;
    let program = read_file(&process.join("comm"))?.trim().to_string();
    let statm = process.join("statm");
    let pages = read_file(&statm)?;
    let mut fields = pages.split_whitespace();
    fields.next();
    let resident = fields
        .next()
        .ok_or_else(|| Unread::Refused(format!("{} has no resident field", statm.display())))
        .and_then(|resident| {
            number(resident, &statm.display().to_string()).map_err(Unread::Refused)
        })?;
    let io = process.join("io");
    let disk = match read_file(&io) {
        Ok(text) => Ok(DiskBytes {
            read: field(&text, "read_bytes:", &io).map_err(Unread::Refused)?,
            written: field(&text, "write_bytes:", &io).map_err(Unread::Refused)?,
        }),
        Err(Unread::Ended) => return Err(Unread::Ended),
        Err(Unread::Refused(error)) => Err(error),
    };
    Ok(Counters {
        program,
        started: None,
        cpu_ns,
        children_cpu_ns: None,
        resident_bytes: resident * page,
        disk,
    })
}

/// Read every process `/proc` lists. A process that ended between the
/// listing and its read has no counters left and is not in the reading; a
/// refused read is kept by pid with its error.
pub(super) fn read() -> Result<Reading, String> {
    let page = page_size()?;
    let mut reading = Reading::default();
    let listing =
        std::fs::read_dir("/proc").map_err(|error| format!("cannot list /proc: {error}"))?;
    for entry in listing {
        let entry = entry.map_err(|error| format!("cannot list /proc: {error}"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let pid: i32 = name
            .parse()
            .map_err(|error| format!("/proc/{name} is not a pid: {error}"))?;
        match counters(&entry.path(), page) {
            Ok(counters) => {
                reading.processes.insert(pid, counters);
            }
            Err(Unread::Ended) => {}
            Err(Unread::Refused(error)) => {
                reading.unreadable.insert(pid, error);
            }
        }
    }
    Ok(reading)
}
