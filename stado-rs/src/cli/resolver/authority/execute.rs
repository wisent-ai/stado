//! Finite authority commands share native SSH authentication, not a local process.

use anyhow::{bail, Context, Result};
use russh::ChannelMsg;

use crate::deploy::host_access::native;

pub(crate) struct Output {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_status: Option<u32>,
    pub(crate) stdout_exceeded: bool,
}

pub(crate) async fn execute(destination: &str, command: &str, limit: usize) -> Result<Output> {
    let session = native::connect(destination).await?;
    let mut channel = session.channel_open_session().await.context("open authority command channel")?;
    channel.exec(true, command).await.context("request authority command execution")?;
    let mut output = Output {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_status: None,
        stdout_exceeded: false,
    };
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => {
                if data.len() > limit.saturating_sub(output.stdout.len()) {
                    output.stdout_exceeded = true;
                    channel.close().await.context("close oversized authority response")?;
                    return Ok(output);
                }
                output.stdout.extend_from_slice(&data);
            }
            ChannelMsg::ExtendedData { data, .. } => {
                let keep = data.len().min(limit.saturating_sub(output.stderr.len()));
                output.stderr.extend_from_slice(&data[..keep]);
            }
            ChannelMsg::ExitStatus { exit_status } => output.exit_status = Some(exit_status),
            ChannelMsg::Failure => bail!("SSH server refused the authority command"),
            _ => {}
        }
    }
    Ok(output)
}
