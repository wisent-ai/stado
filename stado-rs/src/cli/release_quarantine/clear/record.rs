//! Where a clearing is written down and how the displaced state file is
//! named.

use chrono::Utc;

/// A filename-safe instant, so a backup sorts by age and never collides with
/// the one before it.
pub(super) fn stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
