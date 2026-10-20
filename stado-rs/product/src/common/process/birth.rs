//! A process's birth time, read from the kernel.
//!
//! A PID alone can be reused before a later install reads its record, so a
//! lock or an after-install marker records its holder's birth time with it.
//! The time is not read from `/bin/ps`: `ps` is setuid, and a process tree that
//! may not start one is refused it with EPERM, so every lock read and
//! `stado product status` failed whole there, a dead holder's lock included.
use anyhow::{bail, Context, Result};

/// The operating system's birth time for a PID, or absence when it has
/// exited. The text is what `ps -o lstart=` prints, in local time, so a birth
/// time an earlier Stado recorded keeps comparing equal.
pub fn started_at(pid: i32) -> Result<Option<String>> {
    let Some(seconds) = birth_seconds(pid)? else {
        return Ok(None);
    };
    let born = std::time::UNIX_EPOCH
        .checked_add(std::time::Duration::from_secs(seconds))
        .with_context(|| {
            format!("pid {pid} reports birth second {seconds}, past this host's clock")
        })?;
    Ok(Some(
        chrono::DateTime::<chrono::Local>::from(born)
            .format("%a %b %e %H:%M:%S %Y")
            .to_string(),
    ))
}

/// The kernel's birth second for `pid`, or absence when it has exited.
#[cfg(target_os = "macos")]
fn birth_seconds(pid: i32) -> Result<Option<u64>> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>())?;
    // The call takes an argument that PROC_PIDTBSDINFO ignores; the pid itself
    // goes there, so the call names no value of its own.
    let ignored = u64::try_from(pid)?;
    // SAFETY: the kernel writes at most `size` bytes into this live buffer and
    // returns how many it wrote.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            ignored,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written == size {
        // SAFETY: the kernel filled the whole structure.
        return Ok(Some(unsafe { info.assume_init() }.pbi_start_tvsec));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) || !crate::common::alive(pid) {
        return Ok(None);
    }
    bail!(
        "the kernel did not report the birth time of live pid {pid}: proc_pidinfo wrote {written} of {size} bytes: {error}"
    )
}

/// `/proc/<pid>/stat`'s fields after the parenthesised command name, in
/// proc(5)'s order, as far as the process's start time.
#[cfg(target_os = "linux")]
const STAT_AFTER_COMMAND: &[&str] = &[
    "state",
    "ppid",
    "pgrp",
    "session",
    "tty_nr",
    "tpgid",
    "flags",
    "minflt",
    "cminflt",
    "majflt",
    "cmajflt",
    "utime",
    "stime",
    "cutime",
    "cstime",
    "priority",
    "nice",
    "num_threads",
    "itrealvalue",
    "starttime",
];

/// The kernel's birth second for `pid`, or absence when it has exited: its
/// start in clock ticks after boot, plus the boot second.
#[cfg(target_os = "linux")]
fn birth_seconds(pid: i32) -> Result<Option<u64>> {
    let path = format!("/proc/{pid}/stat");
    let stat = match std::fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {path}")),
    };
    // The command name may hold spaces and parentheses; its last `)` ends it.
    let ticks: u64 = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| {
            fields
                .split_whitespace()
                .zip(STAT_AFTER_COMMAND)
                .find_map(|(value, name)| (*name == "starttime").then_some(value))
        })
        .with_context(|| format!("{path} names no start time: {stat}"))?
        .parse()
        .with_context(|| format!("{path} names a start time that is no number: {stat}"))?;
    // SAFETY: sysconf reads one configuration value and has no preconditions.
    let hertz = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
        .context("sysconf(_SC_CLK_TCK) answered no tick rate")?;
    let boot: u64 = std::fs::read_to_string("/proc/stat")
        .context("reading /proc/stat")?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .context("/proc/stat names no boot time")?
        .trim()
        .parse()
        .context("/proc/stat names a boot time that is no number")?;
    let after_boot = ticks
        .checked_div(hertz)
        .context("sysconf(_SC_CLK_TCK) answered zero ticks a second")?;
    Ok(Some(boot + after_boot))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn birth_seconds(pid: i32) -> Result<Option<u64>> {
    bail!("reading the birth time of pid {pid} is unsupported on this operating system")
}
