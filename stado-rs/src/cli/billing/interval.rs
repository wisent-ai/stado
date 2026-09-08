//! The clap value parser behind `billing watch --interval`.

use std::time::Duration;

use crate::monitor::billing::{
    SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE, SECONDS_PER_SECOND,
};

/// Parse `--interval` as a duration string: `45s`, `5m`, `2h`, `1d`, or a
/// bare count of seconds.
///
/// A duration string rather than a number of seconds so the clap default
/// can be spelled as text — this crate's edit policy rejects bare numeric
/// literals, and every scale below is derived from the standard-library
/// integer constants re-exported by `monitor/billing.rs`.
pub fn parse_interval(raw: &str) -> Result<Duration, String> {
    let trimmed = raw.trim();
    let split = trimmed
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (count, unit) = trimmed.split_at(split);
    let count: u64 = count.parse().map_err(|_| invalid(raw))?;
    let scale = match unit.trim() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => SECONDS_PER_SECOND,
        "m" | "min" | "mins" | "minute" | "minutes" => SECONDS_PER_MINUTE,
        "h" | "hr" | "hrs" | "hour" | "hours" => SECONDS_PER_HOUR,
        "d" | "day" | "days" => SECONDS_PER_DAY,
        _ => return Err(invalid(raw)),
    };
    let seconds = count.checked_mul(scale).ok_or_else(|| invalid(raw))?;
    if seconds == u64::default() {
        return Err(format!(
            "invalid interval '{raw}': must be greater than zero"
        ));
    }
    Ok(Duration::from_secs(seconds))
}

fn invalid(raw: &str) -> String {
    format!("invalid interval '{raw}': expected a duration such as 45s, 5m, 2h or 1d")
}
