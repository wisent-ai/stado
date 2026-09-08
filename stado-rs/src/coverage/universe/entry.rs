//! One expected job in a coverage-bound batch.

use serde_json::{Map, Value};

/// One expected job in a coverage-bound batch (Python `UniverseEntry`).
/// `group_key` uniquely identifies the entry within the universe (the
/// state-file key); `command` is the shell command that would produce
/// `expected_uri` when it succeeds; `extra` carries per-entry arguments
/// forwarded to submit on retry (only `verify_command` is consumed here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniverseEntry {
    pub group_key: String,
    pub command: String,
    pub expected_uri: String,
    pub extra: Map<String, Value>,
}

impl UniverseEntry {
    pub fn new(
        group_key: impl Into<String>,
        command: impl Into<String>,
        expected_uri: impl Into<String>,
    ) -> Self {
        Self {
            group_key: group_key.into(),
            command: command.into(),
            expected_uri: expected_uri.into(),
            extra: Map::new(),
        }
    }
}
