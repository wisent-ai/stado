//! The transitions a slot makes: the claim and spawn, the cooperative yield
//! back to the queue, and the tick that carries a live slot to its durable
//! terminal state.
//!
//! `disk_cleanup` is imported here rather than in the parts because the
//! janitor's work-directory creation is named `super::disk_cleanup::...`
//! exactly as it was when this was one file.

use crate::providers::local::disk_cleanup;

use super::*;

mod advance;
mod expiry;
mod start;
mod tick;

use tick::{running_tick, verification_failure};

pub use advance::*;
pub use expiry::*;
pub use start::*;

/// A workstation that goes to sleep takes its running job with it: the
/// process stops, the heartbeat stops, and the queue records `worker lease
/// expired` — which is how a native build dies minutes after the laptop
/// running it enters sleep, hundreds of crates in. The host is interactive
/// by declaration, so
/// sleep is expected; a claimed job is the reason not to. On Darwin the job's
/// lifetime holds an idle-sleep assertion through the system's own
/// `caffeinate`, released the moment the job's pid ends; a closed lid still
/// sleeps, because the operator closing the lid is a decision and idling is
/// not. Other platforms have no such assertion and nothing to hold.
fn hold_awake_while_running(pid: i32, job_id: &str, log_fn: &mut dyn FnMut(&str)) {
    if !cfg!(target_os = "macos") {
        return;
    }
    match std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => log_fn(&format!(
            "holding this host awake while {job_id} runs (caffeinate -i -w {pid})"
        )),
        Err(error) => log_fn(&format!(
            "cannot hold this host awake while {job_id} runs: /usr/bin/caffeinate: {error}; idle sleep will end the job"
        )),
    }
}
