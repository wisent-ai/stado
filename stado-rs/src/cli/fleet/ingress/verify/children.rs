//! The two waits on children this command started: the loopback listener
//! answering its own port, and `cloudflared` printing the address it was given.

use std::path::Path;
use std::process::Child;

use crate::cli::fleet::ingress::{FETCH_TIMEOUT, LISTENER_DEADLINE, POLL, TUNNEL_DEADLINE};

use super::{log_tail, tunnel_address};

/// Wait for the loopback listener to answer its own `/join.sh`.
pub async fn await_listener(child: &mut Child, port: u16, log: &Path) -> Result<(), String> {
    let endpoint = format!("http://127.0.0.1:{port}/join.sh");
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|exc| format!("could not build an HTTP client: {exc}"))?;
    let deadline = tokio::time::Instant::now() + LISTENER_DEADLINE;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the enrollment listener exited immediately ({status}); its log says: {}",
                log_tail(log, 5)
            ));
        }
        if let Ok(response) = client.get(&endpoint).send().await {
            if response.status().as_u16() == 200 {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "the enrollment listener did not answer {endpoint} within {}s; its log says: {}",
                LISTENER_DEADLINE.as_secs(),
                log_tail(log, 5)
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Wait for the tunnel to print the address it was handed.
pub async fn await_tunnel(child: &mut Child, log: &Path) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + TUNNEL_DEADLINE;
    loop {
        if let Some(address) = tunnel_address(log) {
            return Ok(address);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "cloudflared exited before printing an address ({status}); its log says: {}",
                log_tail(log, 5)
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "cloudflared printed no *.trycloudflare.com address within {}s; its log says: {}",
                TUNNEL_DEADLINE.as_secs(),
                log_tail(log, 5)
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}
