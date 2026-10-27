//! Which processes have a lock file open when its record names a holder that
//! is gone.
//!
//! A lock's record is written by the process that took it; the record of a
//! holder that exited stays in the file. The lock itself is a flock on the
//! open file, which the open-file table does not tell apart from a plain
//! open: the process holding the flock and every installation waiting on the
//! same lock have the file open alike. So when the recorded holder no longer
//! runs, the record is called stale and every other process with the file
//! open is named as having it open, the holder among them.

use std::path::Path;
use std::process::Command;

/// Every other process that has `path` open, as `pid <pid> (<command>)`,
/// read with `lsof`; the process asking has it open too and is left out. The
/// sentence says so when no other process has it open or `lsof` cannot say.
pub(super) fn open_by(path: &Path) -> String {
    let output = match stado_wait::output(Command::new("lsof").args(["-F", "pc", "--"]).arg(path)) {
        Ok(output) => output,
        Err(error) => {
            return format!(
            "which processes have the lock file open cannot be read: lsof did not start: {error}"
        )
        }
    };
    let listing = String::from_utf8_lossy(&output.stdout);
    let asking = std::process::id().to_string();
    let mut open = Vec::new();
    let mut pid = None;
    for line in listing.lines() {
        if let Some(found) = line.strip_prefix('p') {
            pid = Some(found.to_owned()).filter(|found| *found != asking);
        } else if let (Some(command), Some(process)) = (line.strip_prefix('c'), pid.take()) {
            open.push(format!("pid {process} ({command})"));
        }
    }
    if !open.is_empty() {
        return format!(
            "these processes have the lock file open, the flock holder among them: {}",
            open.join(", ")
        );
    }
    let said = String::from_utf8_lossy(&output.stderr);
    if said.trim().is_empty() {
        "no other process has the lock file open".to_owned()
    } else {
        format!(
            "which processes have the lock file open cannot be read: lsof said {}",
            said.trim()
        )
    }
}
