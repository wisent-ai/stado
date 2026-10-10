//! Waits on a child process: run a command to its end, or collect a child
//! already started.

use std::fmt::Display;

use crate::{blocking, until, Kind};

/// The program and arguments `command` runs, as a person would type them.
fn command_line(command: &std::process::Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where `command` runs: its directory, or this process's own when none is
/// set.
fn directory(command: &std::process::Command) -> String {
    match command.get_current_dir() {
        Some(directory) => directory.display().to_string(),
        None => "the caller's directory".to_string(),
    }
}

/// Run `command` to its end and collect what it printed, saying so at both
/// ends: what is waited for is its command line, where is the directory it
/// runs in.
pub fn output(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    let place = directory(command);
    blocking(Kind::Process, command_line(command), place, || {
        command.output()
    })
}

/// The same for a `tokio` command: run it to its end and collect what it
/// printed, saying so at both ends.
pub async fn output_async(
    command: &mut tokio::process::Command,
) -> std::io::Result<std::process::Output> {
    let standard = command.as_std();
    let place = directory(standard);
    let what = command_line(standard);
    until(Kind::Process, what, place, command.output()).await
}

/// The waits this function makes, spelled as every product spells its
/// mechanism: the race of the child's exit against the cancellation.
mod wait {
    use std::future::Future;

    pub(super) use crate::until;

    pub(super) async fn until_cancellable<T>(
        cancellation: &crate::Cancellation,
        work: impl Future<Output = T>,
    ) -> Option<T> {
        cancellation.until(work).await
    }
}

/// Reap a killed child: wait for its exit, saying so at both ends. No
/// cancellation ends this wait; a killed child exits.
async fn reap(child: &mut tokio::process::Child) -> std::io::Result<std::process::ExitStatus> {
    let place = match child.id() {
        Some(pid) => format!("pid {pid}"),
        None => "a child that has already ended".to_string(),
    };
    wait::until(
        crate::Kind::Process,
        "reap the killed child",
        place,
        child.wait(),
    )
    .await
}

/// The process group one cancellable child leads, with every local
/// descendant that inherited its pipes. Killing the group is what lets the
/// drain of those pipes end: a descendant left alive would hold the write
/// end open after the direct child is gone. Only Unix gives this crate a
/// group to own; elsewhere a cancellable child is refused before it starts,
/// never answered with a kill that stopped nothing.
struct OwnedGroup {
    leader: u32,
    killed: bool,
}

impl OwnedGroup {
    /// Spawn the child as the leader of a group of its own.
    fn spawn(
        command: &mut tokio::process::Command,
    ) -> std::io::Result<(tokio::process::Child, Self)> {
        #[cfg(not(unix))]
        {
            let _ = command;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "a cancellable child needs a process group of its own to end its descendants, \
                 which only Unix gives this program",
            ))
        }
        #[cfg(unix)]
        {
            // POSIX: setpgid(pid, pgid) with pgid equal to pid makes the
            // process the leader of a new group whose id is its own pid.
            // Done in the child between fork and exec, by its own pid, so
            // the group exists before the program it runs starts anything.
            // SAFETY: getpid and setpgid are async-signal-safe and touch no
            // memory of the parent; pre_exec runs in the forked child.
            unsafe {
                command.pre_exec(|| {
                    let own = libc::getpid();
                    if libc::setpgid(own, own).is_negative() {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command.spawn()?;
            let Some(leader) = child.id() else {
                return Err(std::io::Error::other(
                    "the child ended before its pid could be read, so its group cannot be owned",
                ));
            };
            Ok((
                child,
                Self {
                    leader,
                    killed: false,
                },
            ))
        }
    }

    /// SIGKILL the whole group once, by the leader's id as it was at spawn.
    /// A group already gone (`ESRCH`) is the end wanted.
    fn kill(&mut self) -> std::io::Result<()> {
        if self.killed {
            return Ok(());
        }
        self.killed = true;
        #[cfg(unix)]
        {
            let pid = i32::try_from(self.leader).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("pid {} does not fit a process-group id", self.leader),
                )
            })?;
            // SAFETY: killpg takes a process-group id and a signal; both are
            // plain integers, and the group is the one this process created.
            let answered = unsafe { libc::killpg(pid, libc::SIGKILL) };
            if answered.is_negative() {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!(
                    "the group of pid {} cannot be ended on this platform",
                    self.leader
                ),
            ))
        }
    }
}

/// [`output_async`] that the caller can stop. The child is spawned as the
/// leader of its own process group with its output piped, and stays owned
/// here until it has ended either way: when `cancellation` fires the whole
/// group — the child and every local descendant that inherited its pipes —
/// is sent SIGKILL, the child is reaped, its pipes are read to their end
/// (which the dead group lets happen), and only then does this return the
/// `Interrupted` error naming the cancellation. Nothing of it is left to a
/// drop; the wait line says the end it had (`czekanie odwolane`).
pub async fn output_cancellable(
    command: &mut tokio::process::Command,
    cancellation: &crate::Cancellation,
) -> std::io::Result<std::process::Output> {
    let standard = command.as_std();
    let place = directory(standard);
    let what = command_line(standard);
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, mut group) = OwnedGroup::spawn(command)?;
    let waiting = crate::begin(Kind::Process, what, place);
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // Keep the leader unreaped while descendants may still hold its pipes.
    // Cancellation remains observable throughout communication, not merely
    // until the direct child exits.
    let completed = async {
        let (out, err) = tokio::join!(drain(&mut stdout), drain(&mut stderr));
        let status = wait::until(
            Kind::Process,
            "reap the completed child",
            format!("pid {:?}", child.id()),
            child.wait(),
        )
        .await?;
        Ok::<_, std::io::Error>(std::process::Output {
            status,
            stdout: out?,
            stderr: err?,
        })
    };
    match wait::until_cancellable(cancellation, completed).await {
        Some(output) => waiting.settle(output),
        None => {
            let killed = group.kill();
            if let Err(error) = &killed {
                eprintln!("stado: cancelling owned process group failed: {error}; retaining ownership until it exits");
            }
            let drained = async { tokio::join!(drain(&mut stdout), drain(&mut stderr)) };
            let (reaped, (out, err)) = tokio::join!(reap(&mut child), drained);
            let outcome = (|| {
                killed?;
                reaped?;
                out?;
                err?;
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    crate::CANCELLED,
                ))
            })();
            waiting.settle(outcome)
        }
    }
}

async fn drain<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut Option<R>,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    if let Some(reader) = reader {
        reader.read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

/// Run `command` to its end with its output going where it was told to,
/// saying so at both ends.
pub fn status(command: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    let place = directory(command);
    blocking(Kind::Process, command_line(command), place, || {
        command.status()
    })
}

/// The same for a `tokio` command.
pub async fn status_async(
    command: &mut tokio::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    let standard = command.as_std();
    let place = directory(standard);
    let what = command_line(standard);
    until(Kind::Process, what, place, command.status()).await
}

/// Wait for a child already started — `what` names it, as its caller knows
/// it — and collect what it printed, saying so at both ends.
pub fn child_output(
    child: std::process::Child,
    what: impl Display,
) -> std::io::Result<std::process::Output> {
    let place = format!("pid {}", child.id());
    blocking(Kind::Process, what, place, || child.wait_with_output())
}

/// The same for a `tokio` child already started: `what` names it, as its
/// caller knows it.
pub async fn child_output_async(
    child: tokio::process::Child,
    what: impl Display,
) -> std::io::Result<std::process::Output> {
    let place = match child.id() {
        Some(pid) => format!("pid {pid}"),
        None => "a child that has already ended".to_string(),
    };
    until(Kind::Process, what, place, child.wait_with_output()).await
}
