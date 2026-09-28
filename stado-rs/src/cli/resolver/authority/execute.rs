//! Finite authority commands share native SSH authentication, not a local process.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use russh::ChannelMsg;

use crate::deploy::host_access::native;

/// Tuning constant: seconds one authority command may take from connect to
/// its last byte. The snapshot it carries is at most 1 MiB. Unbounded, one
/// SSH channel that stopped answering held the resolver's refresh for good
/// on 2026-09-28: the generation froze at 99 while the authority published
/// 100, and the route consumer-add had just declared was never bound
/// (defect 27755222). A read past this bound fails like any transport
/// refusal, and the refresh loop backs off onto a fresh session.
const AUTHORITY_COMMAND_SECONDS: u64 = 60;

pub(crate) struct Output {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_status: Option<u32>,
    pub(crate) stdout_exceeded: bool,
}

pub(crate) async fn execute(destination: &str, command: &str, limit: usize) -> Result<Output> {
    let bound = Duration::from_secs(AUTHORITY_COMMAND_SECONDS);
    match tokio::time::timeout(bound, run(destination, command, limit)).await {
        Ok(result) => result,
        Err(_) => bail!(
            "authority command on {destination} gave no complete answer within \
             {AUTHORITY_COMMAND_SECONDS}s"
        ),
    }
}

async fn run(destination: &str, command: &str, limit: usize) -> Result<Output> {
    let session = native::connect(destination).await?;
    let mut channel = session
        .channel_open_session()
        .await
        .context("open authority command channel")?;
    channel
        .exec(true, command)
        .await
        .context("request authority command execution")?;
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
                    channel
                        .close()
                        .await
                        .context("close oversized authority response")?;
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
