//! Who cleared it, where that is written down, and how the displaced state
//! file is named.

use chrono::Utc;

/// `$USER`, the identity every other operator-initiated record in this CLI is
/// stamped with.
pub(super) fn actor() -> String {
    std::env::var("USER").unwrap_or_else(|_| "operator".to_string())
}

/// The append-only record of every quarantine an operator retired, beside the
/// state it changed.
///
/// This area had no audit trail because it had no mutating command: the only
/// way to clear a quarantine was an editor on the host, which leaves nothing
/// behind at all. One JSONL line per clear, next to the document it changed, so
/// the next reader of that state file finds the account of why it looks the way
/// it does without leaving the directory.
pub(super) fn audit_path(state_dir: &str, product: &str) -> String {
    format!("{state_dir}/{product}.quarantine-audit.jsonl")
}

/// A filename-safe instant, so a backup sorts by age and never collides with
/// the one before it.
pub(super) fn stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}
