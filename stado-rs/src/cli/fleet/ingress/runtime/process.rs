//! Processes: where the two children's output goes, how they are started so
//! they outlive the command, and how they are taken away again without
//! signalling something that merely inherited their pid.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;

use crate::cli::fleet::ingress::{POLL, TERMINATE_GRACE};

/// Directory the two children's output goes to, following the same
/// `$HOME/.stado/<thing>` layout the rest of the installation uses.
pub fn runtime_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    let directory = Path::new(&home).join(".stado").join("ingress");
    std::fs::create_dir_all(&directory).map_err(|exc| {
        format!(
            "could not create the ingress log directory {}: {exc}",
            directory.display()
        )
    })?;
    Ok(directory)
}

/// Start one child as its own process-group leader, with its output going to a
/// file rather than to a pipe.
///
/// A pipe would be the obvious way to read `cloudflared`'s address, and it is
/// the wrong one: this command exits while the child keeps running, and a child
/// writing into a pipe nobody drains eventually blocks on its own logging. A
/// file has no reader to lose.
pub fn spawn_detached(program: &Path, args: &[String], log: &Path) -> Result<Child, String> {
    let file = std::fs::File::create(log)
        .map_err(|exc| format!("could not open {} for writing: {exc}", log.display()))?;
    let errors = file.try_clone().map_err(|exc| {
        format!(
            "could not duplicate the log handle for {}: {exc}",
            log.display()
        )
    })?;
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors))
        // Leader of a fresh group: the pid is the group id, the group is what
        // `down` signals, and a Ctrl-C in the terminal that ran `up` is aimed
        // at the foreground group this child is deliberately not in.
        .process_group(0)
        .spawn()
        .map_err(|exc| format!("could not start {}: {exc}", program.display()))
}

/// The command line of a live process, or `None` when there is none. Used to
/// refuse to signal a pid that has been recycled into something else.
fn process_command(pid: i32) -> Option<String> {
    let output = Command::new("/bin/ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Whether the leader of this group is still the process that was started.
pub fn group_alive(pgid: i32, marker: &str) -> bool {
    process_command(pgid).is_some_and(|command| command.contains(marker))
}

/// Signal a whole process group away, refusing to touch a pid that no longer
/// looks like what it was. Returns whether anything was actually signalled.
pub fn terminate_group(pgid: i32, marker: &str) -> bool {
    if !group_alive(pgid, marker) {
        return false;
    }
    let group = Pid::from_raw(pgid);
    let _ = killpg(group, Signal::SIGTERM);
    let deadline = std::time::Instant::now() + TERMINATE_GRACE;
    while std::time::Instant::now() < deadline {
        if process_command(pgid).is_none() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    let _ = killpg(group, Signal::SIGKILL);
    true
}

/// Stop a child this process started, and reap it.
///
/// The reaping is not tidiness. A killed child of a still-running parent stays
/// in the process table as a zombie: `ps` keeps printing it, so
/// [`terminate_group`]'s "has it gone?" poll would never succeed, burn its
/// whole grace period, and end in a pointless `SIGKILL` — and an operator
/// running `ps` in the middle of a failed `up` would see the process the
/// command just claimed to have stopped. Waiting on the handle we still hold
/// answers the question exactly instead of inferring it.
pub fn terminate_child(child: &mut Child, marker: &str) {
    let group = Pid::from_raw(child.id() as i32);
    if group_alive(child.id() as i32, marker) {
        let _ = killpg(group, Signal::SIGTERM);
    }
    let deadline = std::time::Instant::now() + TERMINATE_GRACE;
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL);
    }
    let _ = killpg(group, Signal::SIGKILL);
    let _ = child.wait();
}

/// One error plus everything underneath it.
///
/// `reqwest`'s own `Display` is "error sending request" for every transport
/// failure there is — a refused connection, an unresolved name and a rejected
/// certificate all read identically, which is useless in a message whose whole
/// job is to say what went wrong out on the network. The causes carry the
/// answer, so the message carries the causes.
pub fn with_causes(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}
