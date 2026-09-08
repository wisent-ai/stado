//! Waiting, and the one thing that decides: nothing is published until the
//! address has served this build's own `/join.sh` to a request that left this
//! machine.
//!
//! The two log readers live here because both child waits need them — the
//! tunnel's address is only ever known from what the tunnel printed, and every
//! deadline in this component reports what the child itself said rather than
//! the fact that a timer expired.

use std::io::Read;
use std::path::Path;

pub(in crate::cli::fleet::ingress) mod children;
pub(in crate::cli::fleet::ingress) mod dns;
pub(in crate::cli::fleet::ingress) mod public;

/// The address `cloudflared` printed, if it has printed one yet.
fn tunnel_address(log: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(log)
        .ok()?
        .read_to_string(&mut text)
        .ok()?;
    let pattern = regex::Regex::new(r"https://[a-z0-9][a-z0-9-]*\.trycloudflare\.com").ok()?;
    pattern.find(&text).map(|found| found.as_str().to_string())
}

/// Last few lines of a child's log, for an error that has to say what the child
/// itself said.
fn log_tail(log: &Path, lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(log) else {
        return String::new();
    };
    let collected: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = collected.len().saturating_sub(lines);
    collected[start..].join(" | ")
}
