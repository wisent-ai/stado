//! A process's birth second, read from the kernel.
//!
//! A pid alone names whichever process holds it now: one that ended can have
//! its pid handed to another before anybody reads the record it left. Every
//! record that names a process by pid (a wait in the register, a holding, a
//! product lock) is read against the birth of the process holding that pid
//! now. The time is not read from `/bin/ps`: `ps` is setuid, and a process
//! tree that may not start one is refused it with EPERM.

/// The kernel's birth second for `pid`, or absence when no process holds it.
#[cfg(target_os = "macos")]
pub fn birth_seconds(pid: i32) -> Result<Option<u64>, String> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .map_err(|error| format!("the size of proc_bsdinfo is no C int: {error}"))?;
    // The call takes an argument that PROC_PIDTBSDINFO ignores; the pid itself
    // goes there, so the call names no value of its own.
    let ignored =
        u64::try_from(pid).map_err(|error| format!("pid {pid} is no process id: {error}"))?;
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
    let exists = u32::try_from(pid).is_ok_and(super::running);
    if error.raw_os_error() == Some(libc::ESRCH) || !exists {
        return Ok(None);
    }
    Err(format!(
        "the kernel did not report the birth time of live pid {pid}: proc_pidinfo wrote {written} of {size} bytes: {error}"
    ))
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

/// The kernel's birth second for `pid`, or absence when no process holds it:
/// its start in clock ticks after boot, plus the boot second.
#[cfg(target_os = "linux")]
pub fn birth_seconds(pid: i32) -> Result<Option<u64>, String> {
    let path = format!("/proc/{pid}/stat");
    let stat = match std::fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading {path}: {error}")),
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
        .ok_or_else(|| format!("{path} names no start time: {stat}"))?
        .parse()
        .map_err(|error| {
            format!("{path} names a start time that is no number ({error}): {stat}")
        })?;
    // SAFETY: sysconf reads one configuration value and has no preconditions.
    let hertz = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
        .map_err(|error| format!("sysconf(_SC_CLK_TCK) answered no tick rate: {error}"))?;
    let boot: u64 = std::fs::read_to_string("/proc/stat")
        .map_err(|error| format!("reading /proc/stat: {error}"))?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .ok_or_else(|| "/proc/stat names no boot time".to_string())?
        .trim()
        .parse()
        .map_err(|error| format!("/proc/stat names a boot time that is no number: {error}"))?;
    let after_boot = ticks
        .checked_div(hertz)
        .ok_or_else(|| "sysconf(_SC_CLK_TCK) answered zero ticks a second".to_string())?;
    Ok(Some(boot + after_boot))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn birth_seconds(pid: i32) -> Result<Option<u64>, String> {
    Err(format!(
        "reading the birth time of pid {pid} is unsupported on this operating system"
    ))
}
