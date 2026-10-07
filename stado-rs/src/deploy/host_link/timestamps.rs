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

/// Parse the timestamp spellings the platform log tools print. Anything else
/// is skipped: a line whose time cannot be read is not evidence.
pub(super) fn parse_stamp(raw: &str) -> Option<DateTime<FixedOffset>> {
    const FORMATS: [&str; 6] = [
        // pmset -g log: local date and time with a numeric UTC offset.
        "%Y-%m-%d %H:%M:%S %z",
        // log show --style ndjson: fractional seconds and an unseparated offset.
        "%Y-%m-%d %H:%M:%S%.f%z",
        // journalctl -o short-iso: ISO date separator and colon-separated offset.
        "%Y-%m-%dT%H:%M:%S%:z",
        // journalctl -o short-iso where the offset carries no colon
        "%Y-%m-%dT%H:%M:%S%z",
        // journalctl -o short-iso-precise, both offset spellings
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
    ];
    FORMATS
        .iter()
        .find_map(|format| DateTime::parse_from_str(raw.trim(), format).ok())
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
