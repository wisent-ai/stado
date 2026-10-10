//! Finite authority commands share native SSH authentication, not a local process.

use anyhow::{bail, Context, Result};
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
    let waiting = crate::wait::begin(
        crate::wait::Kind::Host,
        command,
        format!("SSH to {destination}"),
    );
    let result = run(destination, command, input).await;
    match &result {
        Ok(_) => waiting.done(),
        Err(error) => waiting.failed(format!("{error:#}")),
    }
    result
}

async fn run(destination: &str, command: &str, input: Option<&[u8]>) -> Result<Output> {
    let session = native::connect(destination).await?;
    let mut channel = session
        .channel_open_session()
        .await
        .context("open authority command channel")?;
    channel
        .exec(true, command)
        .await
        .context("request authority command execution")?;
    if let Some(input) = input {
        channel
            .data(input)
            .await
            .context("write the authority command's standard input")?;
        channel
            .eof()
            .await
            .context("close the authority command's standard input")?;
    }
    let mut output = Output {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_status: None,
    };
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => output.stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => output.exit_status = Some(exit_status),
            ChannelMsg::Failure => bail!("SSH server refused the authority command"),
            _ => {}
        }
    }
    Ok(output)
}
