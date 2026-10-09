//! The two stages on children this command started: the loopback listener
//! answering on the socket it was handed, and `cloudflared` printing the
//! address it was given and registering a connection for it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStderr, Command, Stdio};

use crate::cli::fleet::ingress::runtime::process::with_causes;

use super::log_tail;

/// What `cloudflared` prints once the edge has accepted a connection for the
/// tunnel; the address it printed before that line is then served.
const REGISTERED: &str = "Registered tunnel connection";

/// Ask the loopback listener for its own `/join.sh`.
///
/// The port is a socket this command bound and handed to the listener, so the
/// connection is accepted by the kernel at once and the request is answered
/// when the listener serves it. A listener that exited closes the socket, and
/// the request fails with the transport's own error.
pub async fn await_listener(child: &mut Child, port: u16, log: &Path) -> Result<(), String> {
    let endpoint = format!("http://127.0.0.1:{port}/join.sh");
    let exited = |child: &mut Child| match child.try_wait() {
        Ok(Some(status)) => format!("the listener exited ({status})"),
        Ok(None) => "the listener is still running".to_string(),
        Err(exc) => format!("the listener's state could not be read ({exc})"),
    };
    match reqwest::get(&endpoint).await {
        Ok(response) if response.status() == reqwest::StatusCode::OK => Ok(()),
        Ok(response) => Err(format!(
            "the enrollment listener answered {endpoint} with HTTP {}; its log says: {}",
            response.status(),
            log_tail(log)
        )),
        Err(exc) => Err(format!(
            "the enrollment listener did not answer {endpoint} ({}); {}; its log says: {}",
            with_causes(&exc.without_url()),
            exited(child),
            log_tail(log)
        )),
    }
}

/// Read `cloudflared`'s stderr until it has printed its `*.trycloudflare.com`
/// address and registered a connection for it, copying every line to the log.
///
/// The pipe is then handed to `/bin/cat` in `cloudflared`'s own process group,
/// appending to the same log: `cloudflared` keeps writing after this command
/// exits, a pipe nobody reads would block it, and `down` signals the group, so
/// the drain stops with the tunnel. End of file before both lines means
/// `cloudflared` exited, and the error carries what it printed.
pub fn await_tunnel(child: &mut Child, stderr: ChildStderr, log: &Path) -> Result<String, String> {
    let pattern = regex::Regex::new(r"https://[a-z0-9][a-z0-9-]*\.trycloudflare\.com")
        .map_err(|exc| format!("the tunnel address pattern does not compile: {exc}"))?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log)
        .map_err(|exc| format!("could not open {} for appending: {exc}", log.display()))?;
    let mut reader = BufReader::new(stderr);
    let mut address = None;
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|exc| format!("reading cloudflared's output failed: {exc}"))?;
        if read == 0 {
            let pid = child.id();
            let status = crate::wait::blocking(
                crate::wait::Kind::Process,
                "cloudflared, whose output ended, to exit",
                format!("pid {pid}"),
                || child.wait(),
            )
            .map(|status| status.to_string())
            .unwrap_or_else(|exc| format!("state unreadable: {exc}"));
            return Err(match address {
                None => format!(
                    "cloudflared exited before printing an address ({status}); its log says: {}",
                    log_tail(log)
                ),
                Some(address) => format!(
                    "cloudflared printed {address} and exited before registering a connection \
                     ({status}); its log says: {}",
                    log_tail(log)
                ),
            });
        }
        file.write_all(line.as_bytes())
            .map_err(|exc| format!("could not append to {}: {exc}", log.display()))?;
        if address.is_none() {
            address = pattern.find(&line).map(|found| found.as_str().to_string());
        }
        if let Some(found) = address.as_ref().filter(|_| line.contains(REGISTERED)) {
            let found = found.clone();
            file.write_all(reader.buffer())
                .map_err(|exc| format!("could not append to {}: {exc}", log.display()))?;
            Command::new("/bin/cat")
                .stdin(Stdio::from(reader.into_inner()))
                .stdout(Stdio::from(file))
                .stderr(Stdio::null())
                .process_group(child.id() as i32)
                .spawn()
                .map_err(|exc| {
                    format!("could not start the drain for cloudflared's output: {exc}")
                })?;
            return Ok(found);
        }
    }
}
