//! Timestamps: the fleet's own spelling, the spellings the platform log
//! tools print, and the newest changes one beacon carries.

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};

use super::{InterfaceChange, MAX_DETAIL_CHARS, MAX_INTERFACE_CHANGES};

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
        // pmset -g log: `2026-08-17 09:23:10 -0700`
        "%Y-%m-%d %H:%M:%S %z",
        // log show --style ndjson: `2026-08-19 11:59:32.869840-0700`
        "%Y-%m-%d %H:%M:%S%.f%z",
        // journalctl -o short-iso, as ubuntu-server spells it:
        // `2026-08-17T19:46:46+00:00`
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

/// One log line's own sentence, flattened and truncated.
pub(super) fn detail_of(raw: &str) -> String {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_DETAIL_CHARS {
        return flat;
    }
    flat.chars().take(MAX_DETAIL_CHARS).collect()
}

/// Keep the newest [`MAX_INTERFACE_CHANGES`], in log order.
pub(super) fn newest_changes(
    mut changes: Vec<(DateTime<FixedOffset>, String)>,
) -> Vec<InterfaceChange> {
    changes.sort_by_key(|(stamp, _)| *stamp);
    let start = changes.len().saturating_sub(MAX_INTERFACE_CHANGES);
    changes
        .drain(start..)
        .map(|(stamp, detail)| InterfaceChange {
            at: iso(stamp),
            detail,
        })
        .collect()
}
