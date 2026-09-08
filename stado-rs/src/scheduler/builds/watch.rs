//! The head watch: what revision a recipe's ref points at right now, and
//! whether this recipe is due to be asked again.
//!
//! Both halves exist to keep a short coordinator tick from turning into a
//! `git ls-remote` storm: the pacing stamp is taken before the remote is
//! asked, so a failing remote is retried on the recipe's own cadence.

use std::time::{Duration, Instant};

use super::POLL_STATE;

/// `git ls-remote` wall-clock budget. A wedged remote (credential prompt,
/// dead host) must cost one recipe one line, not stall the tick daemon.
const LS_REMOTE_TIMEOUT_SECONDS: u64 = 30;

/// `git ls-remote <repo> <ref>` -> the remote sha, under a hard timeout and
/// with credential prompts disabled (an unauthenticated private repo must
/// fail, not hang).
pub(super) async fn ls_remote(repo: &str, branch: &str) -> Result<String, String> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["ls-remote", "--", repo, branch])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    let output = tokio::time::timeout(
        Duration::from_secs(LS_REMOTE_TIMEOUT_SECONDS),
        command.output(),
    )
    .await
    .map_err(|_| format!("git ls-remote timed out after {LS_REMOTE_TIMEOUT_SECONDS}s"))?
    .map_err(|exc| format!("git ls-remote spawn failed: {exc}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git ls-remote failed: {}",
            stderr.trim().lines().next().unwrap_or("(no stderr)")
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sha = stdout
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("")
        .to_string();
    if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("ref {branch:?} not found on remote"));
    }
    Ok(sha)
}

/// True when this recipe's own cadence has elapsed (and stamp the attempt).
/// Stamped before the ls-remote so a failing remote is retried on the
/// recipe's cadence, not on every pass.
pub(super) fn recipe_due(name: &str, interval_seconds: u64) -> bool {
    let mut state = POLL_STATE.lock().expect("build poll state lock");
    let due = state
        .last_recipe_poll
        .get(name)
        .is_none_or(|last| last.elapsed() >= Duration::from_secs(interval_seconds.max(1)));
    if due {
        state
            .last_recipe_poll
            .insert(name.to_string(), Instant::now());
    }
    due
}
