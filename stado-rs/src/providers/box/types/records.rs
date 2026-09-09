//! The Box record families: box, account limits, command, prompt, events.
//!
//! Python `BoxInfo`, `BoxLimits`, `BoxCommandResult`, `BoxPromptRun` and
//! `BoxEventPage` — frozen dataclasses ported as plain structs with public
//! fields, filled by the `client` sibling and by `payload::parse`.

use serde_json::{Map, Value};

/// Python `BoxInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxInfo {
    pub box_id: String,
    pub name: String,
    pub state: String,
    pub ip: String,
    pub url: String,
    pub subdomain: String,
    pub created_at: String,
    pub updated_at: String,
    pub archive_after: String,
    pub snapshot_available: bool,
    pub snapshot_completed_at: String,
    pub last_snapshot_attempt_at: String,
    pub last_snapshot_status: String,
}

/// Python `BoxLimits`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxLimits {
    pub can_start: bool,
    pub active_boxes: i64,
    pub max_active_boxes: i64,
    pub billing_status: String,
    pub blocked_reason: String,
    pub credit_balance_seconds: i64,
}

/// Python `BoxCommandResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxCommandResult {
    pub success: bool,
    pub exit_code: Option<i64>,
    pub signal: String,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
}

/// Python `BoxPromptRun`; `raw` is the unmodified `promptRun` dict.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxPromptRun {
    pub prompt_id: String,
    pub status: String,
    pub done: bool,
    pub raw: Map<String, Value>,
}

/// Python `BoxEventPage`.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxEventPage {
    pub events: Vec<Map<String, Value>>,
    pub next_cursor: String,
    pub has_more: bool,
}
