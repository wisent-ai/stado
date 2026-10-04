//! Who holds one TCP port on a managed host.
//!
//! A unit that cannot start because its port is taken is one of the few
//! faults the fleet could not diagnose through the product. A unit can exit 1
//! with `bind 127.0.0.1:<port>: Address already in use` on every spawn while
//! `stado service verify` calls the same port `unreachable (Connection reset
//! by peer)` and `stado service reap` keeps every candidate row because a
//! declared label holds it. Without a read of who owns the socket the
//! diagnosis goes by guesswork, and when the unit is the owner vault every
//! credential read on the fleet waits on that guess.
//!
//! This is that missing answer: the listener's pid, user and command, read
//! over the host channel with a fixed read, on macOS and Linux both.

use serde::Serialize;
use serde_json::json;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

#[derive(Serialize)]
pub struct PortHolder {
    pub pid: String,
    pub user: String,
    pub command: String,
}

/// `lsof` is present on macOS by default and on these Linux hosts; `ss` is
/// the Linux answer when it is not. Both are read-only.
fn reader(port: u32) -> String {
    format!(
        "if command -v lsof >/dev/null 2>&1; then \
           lsof -nP -iTCP:{port} -sTCP:LISTEN; \
         elif command -v ss >/dev/null 2>&1; then \
           ss -ltnp \"sport = :{port}\"; \
         else echo 'no lsof and no ss on this host'; fi"
    )
}

fn parse_lsof(text: &str) -> Vec<PortHolder> {
    text.lines()
        .skip_while(|line| line.starts_with("COMMAND"))
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let command = words.next()?.to_string();
            let pid = words.next()?.to_string();
            let user = words.next()?.to_string();
            Some(PortHolder { pid, user, command })
        })
        .collect()
}

/// Read the process listening on one port of TARGET.
///
/// The answer is the host's own words: the rows the reader printed, plus the
/// raw text, because a shape neither `lsof` nor `ss` produces is still
/// evidence and must not be swallowed by a parser.
pub async fn port_owner(target: &str, port: u32, json: bool) -> Result<(), CmdError> {
    if u16::try_from(port)
        .ok()
        .and_then(std::num::NonZeroU16::new)
        .is_none()
    {
        return Err(CmdError::usage(format!(
            "--port is a TCP port between 1 and {}, not {port}",
            u16::MAX
        )));
    }
    let resolved: ComputeTarget = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    let answered = crate::deploy::host_channel::run_command(&resolved, &reader(port), &runner)
        .await
        .map_err(CmdError::from)?;
    let text = answered.stdout.trim().to_string();
    let holders = parse_lsof(&text);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "port": port,
                "holders": holders,
                "output": text,
            }))?
        );
        return Ok(());
    }
    if holders.is_empty() {
        println!(
            "{}: nothing is listening on port {port}{}",
            resolved.name,
            if text.is_empty() {
                String::new()
            } else {
                format!("; the host said: {text}")
            }
        );
        return Ok(());
    }
    for holder in &holders {
        println!(
            "{}: port {port} is held by pid {} as {} running {}",
            resolved.name, holder.pid, holder.user, holder.command
        );
    }
    Ok(())
}
