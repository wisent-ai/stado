//! Bounded byte chunks, backpressure, and cancellation for an owned CLI child.

use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio_tungstenite::tungstenite::Message;

use super::Socket;

const CHUNK_BYTES: usize = 8192;
const STDOUT: u8 = 1;
const STDERR: u8 = 2;

struct Attachment(Child);

impl Drop for Attachment {
    fn drop(&mut self) {
        if let Some(pid) = self.0.id() {
            #[cfg(unix)]
            {
                use nix::{sys::signal::{killpg, Signal}, unistd::Pid};
                let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGTERM);
            }
            let _ = self.0.start_kill();
        }
    }
}

async fn output(socket: &mut Socket, channel: u8, bytes: &[u8]) -> Result<(), String> {
    let mut frame = Vec::with_capacity(bytes.len() + 1);
    frame.push(channel);
    frame.extend_from_slice(bytes);
    socket.send(Message::binary(frame)).await.map_err(|error| error.to_string())
}

pub(super) async fn run(socket: &mut Socket, arguments: &[String]) -> Result<(), String> {
    let mut command = super::super::execute::command(arguments).map_err(|error| error.message)?;
    command.stdin(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut attachment = Attachment(command.spawn().map_err(|error| format!("could not start workload: {error}"))?);
    let mut stdin = Some(attachment.0.stdin.take().ok_or("workload stdin was not captured")?);
    let mut stdout = attachment.0.stdout.take().ok_or("workload stdout was not captured")?;
    let mut stderr = attachment.0.stderr.take().ok_or("workload stderr was not captured")?;
    let mut out = [0; CHUNK_BYTES];
    let mut err = [0; CHUNK_BYTES];
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut status: Option<std::process::ExitStatus> = None;
    socket.send(Message::text(json!({"type": "attached", "protocol": "stado.workload.v1"}).to_string()))
        .await.map_err(|error| error.to_string())?;
    loop {
        if let Some(status) = status.filter(|_| !stdout_open && !stderr_open) {
            socket.send(Message::text(json!({"type": "exit", "code": status.code(), "ok": status.success()}).to_string()))
                .await.map_err(|error| error.to_string())?;
            return Ok(());
        }
        tokio::select! {
            message = socket.next() => {
                let Some(message) = message else { return Ok(()); };
                let message = message.map_err(|error| error.to_string())?;
                let bytes = match &message {
                    Message::Text(text) => Some(text.as_bytes()),
                    Message::Binary(bytes) if bytes.is_empty() => {
                        if let Some(mut input) = stdin.take() {
                            input.shutdown().await.map_err(|error| error.to_string())?;
                        }
                        continue;
                    }
                    Message::Binary(bytes) => Some(bytes.as_ref()),
                    Message::Close(_) => return Ok(()),
                    _ => None,
                };
                if let Some(bytes) = bytes {
                    stdin.as_mut().ok_or("workload stdin is already closed")?
                        .write_all(bytes).await.map_err(|error| format!("workload stdin write failed: {error}"))?;
                } else {
                    socket.flush().await.map_err(|error| error.to_string())?;
                }
            }
            read = stdout.read(&mut out), if stdout_open => {
                let count = read.map_err(|error| format!("workload stdout read failed: {error}"))?;
                if count == 0 { stdout_open = false; } else { output(socket, STDOUT, &out[..count]).await?; }
            }
            read = stderr.read(&mut err), if stderr_open => {
                let count = read.map_err(|error| format!("workload stderr read failed: {error}"))?;
                if count == 0 { stderr_open = false; } else { output(socket, STDERR, &err[..count]).await?; }
            }
            result = attachment.0.wait(), if status.is_none() => {
                status = Some(result.map_err(|error| format!("workload wait failed: {error}"))?);
            }
        }
    }
}
