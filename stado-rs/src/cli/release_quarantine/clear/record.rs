//! Who cleared it, where that is written down, and how the displaced state
//! file is named.

use chrono::Utc;

/// `$USER`, the identity every other operator-initiated record in this CLI is
/// stamped with.
pub(super) fn actor() -> String {
    std::env::var("USER").unwrap_or_else(|_| "operator".to_string())
}

/// A filename-safe instant, so a backup sorts by age and never collides with
/// the one before it.
pub(super) fn stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
