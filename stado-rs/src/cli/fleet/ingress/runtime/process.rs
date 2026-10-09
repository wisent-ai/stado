//! Processes: where the two children's output goes, how they are started so
//! they outlive the command, and how they are taken away again without
//! signalling something that merely inherited their pid.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;

use crate::cli::CmdError;

/// An operating-system failure of one ingress step, classed by its kind and
/// naming the step.
fn io_step(exc: std::io::Error, step: String) -> CmdError {
    CmdError::from(exc).within(step)
}

/// Directory the two children's output goes to, following the same
/// `$HOME/.stado/<thing>` layout the rest of the installation uses.
pub fn runtime_dir() -> Result<PathBuf, CmdError> {
    let home = std::env::var("HOME").map_err(|_| CmdError::declaration("HOME is not set"))?;
    let directory = Path::new(&home).join(".stado").join("ingress");
    std::fs::create_dir_all(&directory).map_err(|exc| {
        io_step(
            exc,
            format!(
                "could not create the ingress log directory {}",
                directory.display()
            ),
        )
    })?;
    Ok(directory)
}

/// Start one child as its own process-group leader, with `stdin` as given and
/// its output going to a file rather than to a pipe.
///
/// This command exits while the child keeps running, and a child writing into
/// a pipe nobody drains eventually blocks on its own logging. A file has no
/// reader to lose.
pub fn spawn_detached(
    program: &Path,
    args: &[String],
    stdin: Stdio,
    log: &Path,
) -> Result<Child, CmdError> {
    let file = std::fs::File::create(log)
        .map_err(|exc| io_step(exc, format!("could not open {} for writing", log.display())))?;
    let errors = file.try_clone().map_err(|exc| {
        io_step(
            exc,
            format!("could not duplicate the log handle for {}", log.display()),
        )
    })?;
    Command::new(program)
        .args(args)
        .stdin(stdin)
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors))
        // Leader of a fresh group: the pid is the group id, the group is what
        // `down` signals, and a Ctrl-C in the terminal that ran `up` is aimed
        // at the foreground group this child is deliberately not in.
        .process_group(0)
        .spawn()
        .map_err(|exc| io_step(exc, format!("could not start {}", program.display())))
}

/// Start `cloudflared` as its own process-group leader with its stdout going
/// to the log and its stderr — where it prints the address and every
/// connection it registers — piped here, so the tunnel stage reads those lines
/// as they are written. [`crate::cli::fleet::ingress::verify::children`]
/// hands the pipe on to a drain in the same group before this command exits.
pub fn spawn_tunnel(
    program: &Path,
    args: &[String],
    log: &Path,
) -> Result<(Child, ChildStderr), CmdError> {
    let file = std::fs::File::create(log)
        .map_err(|exc| io_step(exc, format!("could not open {} for writing", log.display())))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|exc| io_step(exc, format!("could not start {}", program.display())))?;
    match child.stderr.take() {
        Some(stderr) => Ok((child, stderr)),
        None => Err(CmdError::click(format!(
            "{} started without the stderr pipe it was given",
            program.display()
        ))),
    }
}

/// The command line of a live process, or `None` when there is none. Used to
/// refuse to signal a pid that has been recycled into something else.
fn process_command(pid: i32) -> Option<String> {
    let output = crate::wait::output(Command::new("/bin/ps").args([
        "-o",
        "command=",
        "-p",
        &pid.to_string(),
    ]))
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

/// Signal a whole process group with `SIGTERM`, refusing to touch a pid that
/// no longer looks like what it was. Returns whether anything was signalled.
pub fn terminate_group(pgid: i32, marker: &str) -> Result<bool, crate::cli::CmdError> {
    if !group_alive(pgid, marker) {
        return Ok(false);
    }
    killpg(Pid::from_raw(pgid), Signal::SIGTERM).map_err(|errno| {
        let kind = std::io::Error::from(errno).kind();
        crate::cli::CmdError::click(format!("SIGTERM to process group {pgid} failed: {errno}"))
            .stating(crate::cli::entry::error::io_failure_code(kind))
    })?;
    Ok(true)
}

/// Stop a child this process started, and reap it.
///
/// The reaping is not tidiness. A killed child of a still-running parent stays
/// in the process table as a zombie, so an operator running `ps` in the middle
/// of a failed `up` would see the process the command just claimed to have
/// stopped. Waiting on the handle we still hold answers the question exactly.
pub fn terminate_child(child: &mut Child, marker: &str) {
    let pgid = child.id() as i32;
    if group_alive(pgid, marker) {
        let _ = killpg(Pid::from_raw(pgid), Signal::SIGTERM);
    }
    let _ = crate::wait::blocking(
        crate::wait::Kind::Process,
        format!("{marker} to exit after SIGTERM"),
        format!("pid {pgid}"),
        || child.wait(),
    );
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
