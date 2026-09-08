//! The loopback port the forward binds, the wait for it to accept, and ssh's
//! own last word when it does not.

use std::net::TcpListener;
use std::time::Instant;

use super::super::{FORWARD_DEADLINE, FORWARD_POLL};
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

/// Wait until the forwarded port accepts a connection, or until ssh gives up
/// and says why. A forward that is reported open before anything is listening
/// is how a connection refused ends up looking like a dead API.
pub(super) async fn await_forward(
    child: &mut tokio::process::Child,
    port: u16,
) -> Result<(), DeployError> {
    let deadline = Instant::now() + FORWARD_DEADLINE;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| DeployError(format!("cannot read the SSH forward's state: {error}")))?
        {
            return Err(DeployError(format!(
                "SSH forwarding to the Weles admission API exited ({status}): {}",
                forward_error(child).await
            )));
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(DeployError(format!(
                "SSH forwarding to the Weles admission API did not accept a connection on 127.0.0.1:{port} within {} seconds",
                FORWARD_DEADLINE.as_secs()
            )));
        }
        tokio::time::sleep(FORWARD_POLL).await;
    }
}

/// ssh's own last word, verbatim — a refused key, a rejected bind, a host that
/// is not answering. A paraphrase here would cost the operator the one line
/// that names the cause.
async fn forward_error(child: &mut tokio::process::Child) -> String {
    let Some(mut stderr) = child.stderr.take() else {
        return "ssh forwarding failed".to_string();
    };
    let mut detail = String::new();
    use tokio::io::AsyncReadExt as _;
    if stderr.read_to_string(&mut detail).await.is_err() {
        return "ssh forwarding failed".to_string();
    }
    detail
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or("ssh forwarding failed")
        .to_string()
}
