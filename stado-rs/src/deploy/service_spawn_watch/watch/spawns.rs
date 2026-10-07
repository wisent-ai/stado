//! Opening the channel: input checks, the gap argument, and the one call that
//! puts the fixed script on a host.

use crate::deploy::service::quote_command_match;
use crate::deploy::service_spawn_watch::script::WATCH_SCRIPT;
use crate::deploy::service_spawn_watch::WatchReport;
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::parse::parse_watch;

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
    if seconds == 0 {
        return Err(DeployError(
            "watch length must be a positive number of seconds; 0 watches nothing".to_string(),
        ));
    }
    if interval_ms == 0 {
        return Err(DeployError(
            "sample interval must be a positive number of milliseconds; 0 samples nothing"
                .to_string(),
        ));
    }
    let script = WATCH_SCRIPT
        .replace("@MATCH@", &format!("\"{matched}\""))
        .replace("@SECONDS@", &seconds.to_string())
        .replace("@GAP@", &gap_argument(interval_ms));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError::unreachable(host_channel::last_error_line(
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
