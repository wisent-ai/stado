//! The loopback port the forward binds, the line ssh prints once it listens
//! there, and ssh's own last word when it never does.

use std::net::TcpListener;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::deploy::DeployError;

/// A loopback port the kernel says is free right now.
///
/// A fixed port would collide with the operator's own `host forward-remote`
/// marker and with a second capture run beside this one.
/// `ExitOnForwardFailure=yes` turns the residual race — the port taken between
/// this probe and ssh's bind — into an immediate refusal instead of a silent
/// misroute to whatever took it.
pub(super) fn free_loopback_port() -> Result<u16, DeployError> {
    let listener = TcpListener::bind(("127.0.0.1", u16::default())).map_err(|error| {
        DeployError(format!(
            "cannot reserve a loopback port for the admission forward: {error}"
        ))
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| DeployError(format!("cannot read the reserved loopback port: {error}")))?
        .port();
    Ok(port)
}

/// Wait for ssh (run with `-v`) to say it listens on the forwarded port, or
/// for its stderr to end, which means ssh exited without binding it. The rest
/// of ssh's diagnostics keep being drained so a long-lived forward never
/// blocks on a full pipe.
pub(super) async fn await_forward(
    child: &mut tokio::process::Child,
    port: u16,
) -> Result<(), DeployError> {
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| DeployError("the SSH forward's stderr was not captured".to_string()))?;
    let mut lines = BufReader::new(stderr).lines();
    let listening = format!("Local forwarding listening on 127.0.0.1 port {port}");
    let mut last = String::new();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.contains(&listening) {
                    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
                    return Ok(());
                }
                if !line.trim().is_empty() && !line.starts_with("debug") {
                    last = line;
                }
            }
            Ok(None) => {
                let status = child.wait().await.map_err(|error| {
                    DeployError(format!("cannot read the SSH forward's exit: {error}"))
                })?;
                return Err(DeployError(format!(
                    "SSH forwarding to the Weles admission API exited ({status}) without listening on 127.0.0.1:{port}: {}",
                    if last.is_empty() { "ssh said nothing" } else { &last }
                )));
            }
            Err(error) => {
                return Err(DeployError(format!(
                    "cannot read the SSH forward's output: {error}"
                )))
            }
        }
    }
}
