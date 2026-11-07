//! The owner process a registration names: it must be a running process on
//! this host other than the registering command, or the janitor's next pass
//! takes the tree at once.

use std::io;

use super::{alive, invalid};

/// Refuse an owner PID that cannot keep the tree, each cause in its own
/// words: not a process id at all, this command's own process (which ends
/// when it returns), or a PID no process on this host runs.
pub(super) fn check(owner_pid: i32) -> io::Result<()> {
    if owner_pid <= 0 {
        return Err(invalid(format!(
            "--owner-pid {owner_pid} is not a process id; give the running process of the \
             session that owns the tree"
        )));
    }
    if owner_pid.unsigned_abs() == std::process::id() {
        return Err(invalid(format!(
            "--owner-pid {owner_pid} is this command's own process, which ends when it returns, \
             so the janitor's next pass would take the tree; give the process of the session \
             that owns it (the agent's or job's own, not a shell that ends with the command)"
        )));
    }
    if !alive(owner_pid) {
        return Err(invalid(format!(
            "--owner-pid {owner_pid} names no running process on this host (kill -0 answered no \
             such process), so the janitor's next pass would take the tree; give the running \
             process of the session that owns it"
        )));
    }
    Ok(())
}
