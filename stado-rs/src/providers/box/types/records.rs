//! The Box record families: box, account limits, command, prompt, events.
//!
//! Python `BoxInfo`, `BoxLimits`, `BoxCommandResult`, `BoxPromptRun` and
//! `BoxEventPage` — frozen dataclasses ported as plain structs with public
//! fields, filled by the `client` sibling and by `payload::parse`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::errors::BoxError;

/// A box's lifecycle state as the Box API names it. A state this list does
/// not hold is `Unknown`: neither active nor serving, and the box record
/// keeps the word itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoxState {
    Init,
    Provisioning,
    Provisioned,
    Cloning,
    Ready,
    Idle,
    Running,
    Archiving,
    Archived,
    Error,
    #[serde(other)]
    Unknown,
}

impl BoxState {
    /// The box holds a machine: it is starting, serving or being archived.
    pub fn active(self) -> bool {
        match self {
            Self::Init
            | Self::Provisioning
            | Self::Provisioned
            | Self::Cloning
            | Self::Ready
            | Self::Idle
            | Self::Running
            | Self::Archiving => true,
            Self::Archived | Self::Error | Self::Unknown => false,
        }
    }

    /// The box accepts work now.
    pub fn serving(self) -> bool {
        match self {
            Self::Ready | Self::Idle | Self::Running => true,
            Self::Init
            | Self::Provisioning
            | Self::Provisioned
            | Self::Cloning
            | Self::Archiving
            | Self::Archived
            | Self::Error
            | Self::Unknown => false,
        }
    }

    /// What the state means for the box's life: an archived box has given
    /// its machine back, so it is ended, not stopped.
    pub fn phase(self) -> crate::providers::LifecyclePhase {
        use crate::providers::LifecyclePhase;
        match self {
            Self::Init
            | Self::Provisioning
            | Self::Provisioned
            | Self::Cloning
            | Self::Ready
            | Self::Idle
            | Self::Running => LifecyclePhase::Running,
            Self::Archiving | Self::Archived | Self::Error => LifecyclePhase::Ended,
            Self::Unknown => LifecyclePhase::Unknown,
        }
    }
}

/// Python `BoxInfo`; `state` is the word the Box API sent, `lifecycle()` its
/// meaning.
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

impl BoxInfo {
    /// The box's lifecycle state.
    pub fn lifecycle(&self) -> BoxState {
        match BoxState::deserialize(Value::String(self.state.clone())) {
            Ok(state) => state,
            Err(_) => BoxState::Unknown,
        }
    }
}

/// A prompt run's status as the Box API documents it; any other word is a
/// malformed answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptStatus {
    Sending,
    Queued,
    Running,
    Finished,
    Failed,
}

impl PromptStatus {
    /// The status `word` names, refused naming the word when it is none.
    pub fn read(word: &str) -> Result<Self, BoxError> {
        Self::deserialize(Value::String(word.to_string())).map_err(|error| {
            BoxError::transport(format!("Box prompt status {word:?} is invalid: {error}"))
        })
    }
}

impl std::fmt::Display for PromptStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(Value::String(word)) => formatter.write_str(&word),
            _ => Err(std::fmt::Error),
        }
    }
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
    pub status: PromptStatus,
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
