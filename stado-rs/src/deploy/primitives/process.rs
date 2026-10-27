//! The production runner's process execution: a locally owned process group
//! per invocation and concurrent stdin/stdout/stderr communication, until the
//! command exits.

use super::command::{CommandOutput, CommandSpec};

struct OwnedProcessGroup {
    child_id: Option<u32>,
    armed: bool,
}

impl OwnedProcessGroup {
    fn new(child_id: Option<u32>) -> Self {
        Self {
            child_id,
            armed: true,
        }
    }

    fn terminate(&mut self) -> Option<String> {
        #[cfg(unix)]
        {
            use nix::errno::Errno;
            use nix::sys::signal::{killpg, Signal};
            use nix::unistd::Pid;

            let result = match self.child_id {
                Some(pid) => killpg(Pid::from_raw(pid as i32), Signal::SIGKILL),
                None => {
                    self.armed = false;
                    return Some("child PID unavailable".to_string());
                }
            };
            // One exact attempt owns this process-group identity. Never retry
            // from Drop after reaping, when the numeric id could be recycled.
            self.armed = false;
            match result {
                Ok(()) | Err(Errno::ESRCH) => None,
                Err(error) => Some(error.to_string()),
            }
        }
        #[cfg(not(unix))]
        {
            self.armed = false;
            None
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.terminate();
        }
    }
}

/// Whether a command's output only comes back in its [`CommandOutput`], or
/// is also passed to this process's own stdout and stderr as it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relay {
    /// Captured only: the caller prints what it chooses after the exit.
    Captured,
    /// Captured and written through as each chunk arrives, so a long remote
    /// command is seen working instead of standing silent until it ends.
    Passed,
}

/// Read `reader` to its end into `captured`, writing each chunk through to
/// `sink` as it arrives when there is one. The chunk is whatever the pipe
/// holds when read; nothing waits for a line or a size to fill.
async fn drain<R, W>(reader: R, captured: &mut Vec<u8>, mut sink: Option<W>) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let mut reader = tokio::io::BufReader::new(reader);
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(());
        }
        captured.extend_from_slice(chunk);
        if let Some(sink) = sink.as_mut() {
            sink.write_all(chunk).await?;
            sink.flush().await?;
        }
        let read = chunk.len();
        reader.consume(read);
    }
}

pub(super) async fn run_process(spec: CommandSpec) -> Result<CommandOutput, String> {
    run_process_with(spec, Relay::Captured).await
}

/// [`run_process`] whose output is also written through to this process's
/// stdout and stderr as it arrives (`stado host exec`).
pub(super) async fn run_process_relayed(spec: CommandSpec) -> Result<CommandOutput, String> {
    run_process_with(spec, Relay::Passed).await
}

async fn run_process_with(spec: CommandSpec, relay: Relay) -> Result<CommandOutput, String> {
    use tokio::io::AsyncWriteExt;

    let (program, args) = spec.argv.split_first().ok_or("empty command argv")?;
    let what = spec.argv.join(" ");
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Every invocation owns a fresh local process group. This includes a local
    // shell or ssh client and its local descendants; it does not assert that
    // processes beyond an ssh connection joined that group.
    #[cfg(unix)]
    command.process_group(0);
    if spec.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    } else {
        command.stdin(std::process::Stdio::null());
    }
    let mut child = command.spawn().map_err(|exc| exc.to_string())?;
    // Declared after `child` so cancellation drops this guard first and kills
    // the locally owned group while the leader handle still retains identity.
    let mut owned_group = OwnedProcessGroup::new(child.id());
    let stdin_payload = spec.stdin;
    let stdin_pipe = child.stdin.take();
    let mut stdout = child.stdout.take().ok_or("child stdout was not piped")?;
    let mut stderr = child.stderr.take().ok_or("child stderr was not piped")?;

    // Feed stdin and drain both output pipes concurrently, then reap the direct
    // child, all inside the one announced wait. The command runs until it
    // exits; its exit code and output are the answer, and nothing ends it early.
    let completed = crate::wait::until(
        crate::wait::Kind::Process,
        &what,
        "the caller's directory",
        async {
            let mut stdout_bytes = Vec::new();
            let mut stderr_bytes = Vec::new();
            let write_stdin = async move {
                if let (Some(mut pipe), Some(payload)) = (stdin_pipe, stdin_payload) {
                    if let Err(error) = pipe.write_all(payload.as_bytes()).await {
                        if error.kind() != std::io::ErrorKind::BrokenPipe {
                            return Err(error);
                        }
                    }
                }
                Ok(())
            };
            let (stdout_sink, stderr_sink) = match relay {
                Relay::Captured => (None, None),
                Relay::Passed => (Some(tokio::io::stdout()), Some(tokio::io::stderr())),
            };
            let read_stdout = drain(&mut stdout, &mut stdout_bytes, stdout_sink);
            let read_stderr = drain(&mut stderr, &mut stderr_bytes, stderr_sink);
            tokio::try_join!(write_stdin, read_stdout, read_stderr)?;
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, stdout_bytes, stderr_bytes))
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    owned_group.disarm();
    let (status, stdout, stderr) = completed;
    // A child a signal ended has no exit code; it is said as that, never as a
    // code no process returned.
    let Some(code) = status.code() else {
        return Err(format!(
            "{what} ended with {status} and returned no exit code; stderr: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    };
    Ok(CommandOutput {
        code,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}
