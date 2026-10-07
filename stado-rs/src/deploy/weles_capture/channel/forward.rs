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
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
        let code = crate::cli::entry::error::io_failure_code(error.kind());
        DeployError(format!(
            "cannot reserve a loopback port for the admission forward: {error}"
        ))
        .stating(code)
    })?;
    let port = listener
        .local_addr()
        .map_err(DeployError::io("cannot read the reserved loopback port".to_string()))?
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
        .ok_or_else(|| {
            DeployError("the SSH forward's stderr was not captured".to_string())
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
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
                let status = child
                    .wait()
                    .await
                    .map_err(DeployError::io("cannot read the SSH forward's exit".to_string()))?;
                return Err(DeployError::unreachable(format!(
                    "SSH forwarding to the Weles admission API exited ({status}) without listening on 127.0.0.1:{port}: {}",
                    if last.is_empty() { "ssh said nothing" } else { &last }
                )));
            }
            Err(error) => {
                return Err(DeployError::io(
                    "cannot read the SSH forward's output".to_string(),
                )(error))
            }
        }
    }
}
