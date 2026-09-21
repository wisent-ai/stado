//! Who holds one TCP port on a managed host.
//!
//! A unit that cannot start because its port is taken is one of the few
//! faults this fleet could not diagnose through the product. On 2026-09-21
//! `com.wisent.always-on.skarbiec` on charless-mac-mini exited 1 with
//! `bind 127.0.0.1:8895: Address already in use` on every spawn, while
//! `stado service verify` called the same port `unreachable (Connection reset
//! by peer)` and `stado service reap` kept every candidate row because a
//! declared label held it. Nothing in Stado could say which process owned the
//! socket, so the diagnosis went by guesswork and the owner vault stayed down
//! — and every credential read on the fleet goes through it.
//!
//! This is that missing answer: the listener's pid, user and command, read
//! over the host channel with a fixed read, on macOS and Linux both.

use serde::Serialize;
use serde_json::json;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

/// The highest port number a TCP socket can carry.
const HIGHEST_PORT: u32 = 65_535;

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
    if port == u32::MIN || port > HIGHEST_PORT {
        return Err(CmdError::click(format!(
            "--port is a TCP port between 1 and {HIGHEST_PORT}, not {port}"
        )));
    }
    let resolved: ComputeTarget = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let answered = crate::deploy::host_channel::run_command(&resolved, &reader(port), &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
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

#[cfg(test)]
mod tests {
    use super::{parse_lsof, reader, HIGHEST_PORT};

    const LSOF: &str = "COMMAND   PID    USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME\n\
skarbiec 20542 charles    7u  IPv4 0x9f1a2b3c4d5e6f70      0t0  TCP 127.0.0.1:8895 (LISTEN)";

    #[test]
    fn the_listener_is_read_out_of_the_hosts_own_words() {
        let holders = parse_lsof(LSOF);
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].pid, "20542");
        assert_eq!(holders[0].user, "charles");
        assert_eq!(holders[0].command, "skarbiec");
    }

    #[test]
    fn a_port_nobody_holds_reads_as_no_rows_rather_than_an_error() {
        assert!(parse_lsof("").is_empty());
        assert!(parse_lsof("COMMAND   PID    USER   FD   TYPE").is_empty());
    }

    /// The reader asks for listening sockets only, on both init families, and
    /// says so when the host carries neither tool.
    #[test]
    fn the_reader_names_both_tools_and_the_port_once_each() {
        let command = reader(8895);
        assert!(
            command.contains("lsof -nP -iTCP:8895 -sTCP:LISTEN"),
            "{command}"
        );
        assert!(command.contains("ss -ltnp \"sport = :8895\""), "{command}");
        assert!(command.contains("no lsof and no ss"), "{command}");
        assert_eq!(HIGHEST_PORT, 65_535);
    }
}
