//! Opening the channel: bounds checking, the sleep argument, and the one
//! call that puts the fixed script on a host.

use std::time::Duration;

use crate::deploy::service::quote_command_match;
use crate::deploy::service_spawn_watch::script::WATCH_SCRIPT;
use crate::deploy::service_spawn_watch::{
    WatchReport, MAX_INTERVAL_MS, MAX_SECONDS, MIN_INTERVAL_MS,
};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::parse::parse_watch;

/// Slack added to the watch window for connection setup and teardown, so the
/// channel's own bound never fires before the remote loop has said `DONE`.
const TIMEOUT_SLACK: Duration = Duration::from_secs(60);

/// Render the sleep argument. BSD `sleep` takes a decimal, and a whole number
/// is spelled without a fraction so the common case reads as `1`.
pub(super) fn gap_argument(interval_ms: u64) -> String {
    if interval_ms.is_multiple_of(1000) {
        (interval_ms / 1000).to_string()
    } else {
        format!("{}.{:03}", interval_ms / 1000, interval_ms % 1000)
    }
}

/// Watch one host for processes matching `command_match`, for `seconds`.
///
/// Signals nothing. The only thing this can do to a host is read `ps`.
pub async fn watch_spawns(
    target: &ComputeTarget,
    command_match: &str,
    seconds: u64,
    interval_ms: u64,
    runner: &Runner,
) -> Result<WatchReport, DeployError> {
    let matched = quote_command_match(command_match)?;
    if seconds == 0 || seconds > MAX_SECONDS {
        return Err(DeployError(format!(
            "watch length must be between 1 and {MAX_SECONDS} seconds; {seconds} is outside it"
        )));
    }
    if !(MIN_INTERVAL_MS..=MAX_INTERVAL_MS).contains(&interval_ms) {
        return Err(DeployError(format!(
            "sample interval must be between {MIN_INTERVAL_MS} and {MAX_INTERVAL_MS} ms; \
             {interval_ms} is outside it"
        )));
    }
    let script = WATCH_SCRIPT
        .replace("@MATCH@", &format!("\"{matched}\""))
        .replace("@SECONDS@", &seconds.to_string())
        .replace("@GAP@", &gap_argument(interval_ms));
    let bound = Duration::from_secs(seconds) + TIMEOUT_SLACK;
    let output = host_channel::run_script_with_timeout(target, &script, bound, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the spawn watch did not complete",
        )));
    }
    Ok(parse_watch(
        &target.name,
        &matched,
        seconds,
        interval_ms,
        &output.stdout,
    ))
}
