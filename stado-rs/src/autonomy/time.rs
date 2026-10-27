//! Checked policy durations and the timestamps derived from them.
//!
//! Fitting an integer number of seconds does not prove that either Chrono's
//! duration representation or the resulting calendar timestamp can hold it.

use std::num::NonZeroU64;

use chrono::{DateTime, Duration, Utc};

pub(super) fn positive_duration(seconds: u64, field: &str) -> Result<Duration, String> {
    let positive = NonZeroU64::new(seconds)
        .ok_or_else(|| format!("{field} must be positive; observed {seconds} seconds"))?;
    let signed = i64::try_from(positive.get()).map_err(|error| {
        format!("{field} cannot represent {seconds} seconds as a duration: {error}")
    })?;
    Duration::try_seconds(signed).ok_or_else(|| {
        format!("{field} of {seconds} seconds exceeds the supported duration range")
    })
}

pub(super) fn expires_at(
    now: DateTime<Utc>,
    seconds: u64,
    field: &str,
) -> Result<DateTime<Utc>, String> {
    let duration = positive_duration(seconds, field)?;
    now.checked_add_signed(duration).ok_or_else(|| {
        format!("computing {field} expiry from {now} plus {seconds} seconds exceeds the timestamp range")
    })
}

pub(super) fn lookback_start(
    now: DateTime<Utc>,
    seconds: u64,
    field: &str,
) -> Result<DateTime<Utc>, String> {
    let duration = positive_duration(seconds, field)?;
    now.checked_sub_signed(duration).ok_or_else(|| {
        format!("computing {field} lookback from {now} minus {seconds} seconds exceeds the timestamp range")
    })
}
