//! Finite authority commands share native SSH authentication, not a local process.

use anyhow::Result;
use russh::ChannelMsg;

use crate::deploy::host_access::native;

pub(crate) struct Output {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_status: Option<u32>,
}

/// Run one command over the authority's SSH channel, saying so at both ends
/// (`crate::wait`). The command ends when the channel does; a channel that
/// closes without an exit status is reported as such by the caller. Both
/// streams are kept whole: the 1 MiB ceiling once here would have refused
/// the registry once it grew past it.
pub(crate) async fn execute(destination: &str, command: &str) -> Result<Output> {
    announced(destination, command, None).await
}

/// [`execute`] with `input` written to the command's standard input, which
/// is then closed: how a document reaches a command on the authority without
/// a file there.
pub(crate) async fn execute_with_input(
    destination: &str,
    command: &str,
    input: &[u8],
) -> Result<Output> {
    announced(destination, command, Some(input)).await
}

async fn announced(destination: &str, command: &str, input: Option<&[u8]>) -> Result<Output> {
    // A failed step names itself and its cause, the way the wait's failure
    // line and the caller's refusal read it.
    let failed = |step: &str, cause: String| std::io::Error::other(format!("{step}: {cause}"));
    crate::wait::until(
        crate::wait::Kind::Host,
        command,
        format!("SSH to {destination}"),
        async move {
            let session = native::connect(destination)
                .await
                .map_err(|error| failed("connect to the authority", format!("{error:#}")))?;
            let mut channel = session
                .channel_open_session()
                .await
                .map_err(|error| failed("open authority command channel", error.to_string()))?;
            channel.exec(true, command).await.map_err(|error| {
                failed("request authority command execution", error.to_string())
            })?;
            if let Some(input) = input {
                channel.data(input).await.map_err(|error| {
                    failed(
                        "write the authority command's standard input",
                        error.to_string(),
                    )
                })?;
                channel.eof().await.map_err(|error| {
                    failed(
                        "close the authority command's standard input",
                        error.to_string(),
                    )
                })?;
            }
            let mut output = Output {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_status: None,
            };
            // The command's stderr not yet passed on to this process's
            // stderr. The authority's own wait lines say what it stands on
            // while the caller waits minutes; kept until the command failed,
            // they named nothing while it stood and nothing once it succeeded.
            let mut pending: Vec<u8> = Vec::new();
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => {
                        output.stderr.extend_from_slice(&data);
                        pending.extend_from_slice(&data);
                        let passed = relay_lines(destination, &pending);
                        pending.drain(..passed);
                    }
                    ChannelMsg::ExitStatus { exit_status } => {
                        output.exit_status = Some(exit_status)
                    }
                    ChannelMsg::Failure => {
                        return Err(failed(
                            "run the authority command",
                            "the SSH server refused it".to_string(),
                        ))
                    }
                    _ => {}
                }
            }
            let rest = String::from_utf8_lossy(&pending);
            if !rest.trim().is_empty() {
                eprintln!("{destination}: {}", rest.trim_end());
            }
            Ok(output)
        },
    )
    .await
    .map_err(anyhow::Error::from)
}

/// Write each whole line of `pending` to this process's stderr under the
/// destination's name, and answer how many bytes those lines held; a line
/// whose end has not arrived yet waits for the next chunk.
fn relay_lines(destination: &str, pending: &[u8]) -> usize {
    pending
        .split_inclusive(|byte| *byte == b'\n')
        .take_while(|line| line.ends_with(b"\n"))
        .map(|line| {
            eprintln!(
                "{destination}: {}",
                String::from_utf8_lossy(line).trim_end()
            );
            line.len()
        })
        .sum()
}
