//! What the wait-for graph reads of the machine: the holder a lock file
//! names, a process's command and parent, and the ancestors of a process. A
//! read that fails is the error, so the graph is said unread rather than
//! missing the edge that read would have given.
//!
//! On Linux a process's command and parent are read from its own
//! `/proc/<pid>` files. `ps -p <pid>` there walks every process of the
//! machine to print one: on a host running thousands of threads one call
//! cost over half a second, and the first wait of a command read its own
//! command and five ancestors that way inside the waiting task, so a
//! registry authority read racing its declared paths started them all
//! nearly five seconds late and was abandoned at its five-second bound
//! before any path answered. Elsewhere `ps` reads one process and is kept.

use std::collections::BTreeSet;
use std::path::Path;
#[cfg(not(target_os = "linux"))]
use std::process::Command;

/// The pid a lock file names as its holder (`"pid": <n>`), when that
/// process runs. A lock whose file is gone or names none takes no part in
/// the graph; one that cannot be read, or names a pid that is no number, is
/// the error.
pub(super) fn holder(lock: &Path) -> Result<Option<u32>, String> {
    let text = match std::fs::read_to_string(lock) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading {}: {error}", lock.display())),
    };
    let Some((_, after)) = text.split_once("\"pid\"") else {
        return Ok(None);
    };
    let digits: String = after
        .trim_start()
        .trim_start_matches(':')
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let pid = digits.parse::<u32>().map_err(|error| {
        format!(
            "{} names a holder pid that is no number: {error}",
            lock.display()
        )
    })?;
    Ok(crate::open::running(pid)?.then_some(pid))
}

/// The command `pid` runs, as `ps -o command=` prints it: its arguments
/// joined by spaces, or its name in brackets for a process with none (a
/// kernel thread). None when no such process runs now.
#[cfg(target_os = "linux")]
pub(super) fn command(pid: u32) -> Result<Option<String>, String> {
    let Some(arguments) = proc_file(pid, "cmdline")? else {
        return Ok(None);
    };
    let words: Vec<_> = arguments
        .split(|byte| *byte == 0)
        .filter(|word| !word.is_empty())
        .map(String::from_utf8_lossy)
        .collect();
    if !words.is_empty() {
        return Ok(Some(words.join(" ")));
    }
    Ok(proc_file(pid, "comm")?
        .map(|name| format!("[{}]", String::from_utf8_lossy(&name).trim_end())))
}

/// The parent of `pid`: none when no such process runs now. Its stat line
/// is `pid (name) state ppid ...`, and the name may itself hold spaces and
/// parentheses, so the fields are read after the last `)`.
#[cfg(target_os = "linux")]
fn parent(pid: u32) -> Result<Option<u32>, String> {
    let Some(stat) = proc_file(pid, "stat")? else {
        return Ok(None);
    };
    let stat = String::from_utf8_lossy(&stat);
    let field = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(1))
        .ok_or_else(|| format!("/proc/{pid}/stat names no parent: {stat:?}"))?;
    field
        .parse::<u32>()
        .map(Some)
        .map_err(|error| format!("/proc/{pid}/stat names the parent as {field:?}: {error}"))
}

/// The file `name` of `/proc/<pid>`: none when the process is gone, before
/// the read or during it.
#[cfg(target_os = "linux")]
fn proc_file(pid: u32, name: &str) -> Result<Option<Vec<u8>>, String> {
    let path = format!("/proc/{pid}/{name}");
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(_) if !Path::new(&format!("/proc/{pid}")).exists() => Ok(None),
        Err(error) => Err(format!("reading {path}: {error}")),
    }
}

/// The command `pid` runs (`ps -o command=`): none when `ps` finds no such
/// process now.
#[cfg(not(target_os = "linux"))]
pub(super) fn command(pid: u32) -> Result<Option<String>, String> {
    ps(pid, "command")
}

/// The parent of `pid` (`ps -o ppid=`): none when `ps` finds no such
/// process now.
#[cfg(not(target_os = "linux"))]
fn parent(pid: u32) -> Result<Option<u32>, String> {
    let Some(text) = ps(pid, "ppid")? else {
        return Ok(None);
    };
    text.parse::<u32>()
        .map(Some)
        .map_err(|error| format!("ps names the parent of {pid} as {text:?}: {error}"))
}

/// One `ps` column for `pid`: none when `ps` finds no such process now.
#[cfg(not(target_os = "linux"))]
fn ps(pid: u32, column: &str) -> Result<Option<String>, String> {
    let output = crate::output(Command::new("ps").args([
        "-o",
        &format!("{column}="),
        "-p",
        &pid.to_string(),
    ]))
    .map_err(|error| format!("ps could not be run for process {pid}: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((output.status.success() && !text.is_empty()).then_some(text))
}

/// `pid` and every ancestor of it: the processes whose completion needs it.
pub(super) fn needing(pid: u32) -> Result<BTreeSet<u32>, String> {
    let mut chain = BTreeSet::from([pid]);
    let mut current = pid;
    while let Some(parent) = parent(current)? {
        if !chain.insert(parent) {
            break;
        }
        current = parent;
    }
    Ok(chain)
}

/// A process's command and the chain [`needing`] answers for it.
pub(super) type Lineage = (Option<String>, BTreeSet<u32>);

/// This process's command and its ancestor chain, read once per process:
/// every wait of this process asks the graph about the same chain. An
/// ancestor that exits later leaves its pid in the chain, where it holds
/// nothing.
pub(super) fn this_process() -> Result<&'static Lineage, String> {
    static THIS: std::sync::LazyLock<Result<Lineage, String>> = std::sync::LazyLock::new(|| {
        let pid = std::process::id();
        Ok((command(pid)?, needing(pid)?))
    });
    THIS.as_ref().map_err(Clone::clone)
}
