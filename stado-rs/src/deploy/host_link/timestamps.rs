//! Timestamps: the fleet's own spelling, the spellings the platform log
//! tools print, and the newest changes one beacon carries.

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};

use super::InterfaceChange;

/// One timestamp in the fleet's spelling: UTC, seconds, `Z`.
pub(super) fn iso(stamp: DateTime<FixedOffset>) -> String {
    stamp
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parse the timestamp spellings the macOS log tools print (the Linux
/// journal is read as JSON with its own epoch field). A spelling neither
/// format reads is the error, naming the stamp and each format's reason.
pub(super) fn parse_stamp(raw: &str) -> Result<DateTime<FixedOffset>, String> {
    const FORMATS: &[&str] = &[
        // pmset -g log: local date and time with a numeric UTC offset.
        "%Y-%m-%d %H:%M:%S %z",
        // log show --style ndjson: fractional seconds and an unseparated offset.
        "%Y-%m-%d %H:%M:%S%.f%z",
    ];
    let mut reasons = Vec::new();
    for format in FORMATS {
        match DateTime::parse_from_str(raw.trim(), format) {
            Ok(stamp) => return Ok(stamp),
            Err(error) => reasons.push(format!("{format}: {error}")),
        }
    }
    Err(format!(
        "{raw:?} is no timestamp a log tool prints ({})",
        reasons.join("; ")
    ))
}

/// One log line's own sentence, flattened.
pub(super) fn detail_of(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every change inside the window, in log order. The window is one beacon
/// interval, so time already bounds the list; no count is chosen here.
pub(super) fn newest_changes(
    mut changes: Vec<(DateTime<FixedOffset>, String)>,
) -> Vec<InterfaceChange> {
    changes.sort_by_key(|(stamp, _)| *stamp);
    changes
        .into_iter()
        .map(|(stamp, detail)| InterfaceChange {
            at: iso(stamp),
            detail,
        })
        .collect()
}
