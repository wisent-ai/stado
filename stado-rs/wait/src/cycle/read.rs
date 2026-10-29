//! What the wait-for graph reads of the machine: the holder a lock file
//! names, a process's `ps` columns, and the ancestors of a process. A read
//! that fails is the error, so the graph is said unread rather than missing
//! the edge that read would have given.

use std::collections::BTreeSet;
use std::path::Path;
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
    Ok(crate::open::running(pid).then_some(pid))
}

/// One `ps` column for `pid`: none when `ps` finds no such process now.
pub(super) fn ps(pid: u32, column: &str) -> Result<Option<String>, String> {
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
    while let Some(text) = ps(current, "ppid")? {
        let parent = text
            .parse::<u32>()
            .map_err(|error| format!("ps names the parent of {current} as {text:?}: {error}"))?;
        if !chain.insert(parent) {
            break;
        }
        current = parent;
    }
    Ok(chain)
}

/// A process's `ps` command and the chain [`needing`] answers for it.
pub(super) type Lineage = (Option<String>, BTreeSet<u32>);

/// This process's command and its ancestor chain, read once per process.
/// Every wait of this process asks the graph about the same chain, and each
/// read is one `ps` per ancestor run inside the waiting task: on a busy host
/// that cost seconds per wait, so concurrent waits of one task (an authority
/// read racing its declared paths) started one after another and the read
/// missed its bound before its last path was tried. An ancestor that exits
/// later leaves its pid in the chain, where it holds nothing.
pub(super) fn this_process() -> Result<&'static Lineage, String> {
    static THIS: std::sync::LazyLock<Result<Lineage, String>> = std::sync::LazyLock::new(|| {
        let pid = std::process::id();
        Ok((ps(pid, "command")?, needing(pid)?))
    });
    THIS.as_ref().map_err(Clone::clone)
}
