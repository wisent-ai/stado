//! What one host answered about its own periodic table, and the one command
//! that puts back whatever `--apply` changed.

use crate::deploy::shlex_quote;

use super::{STATE_ABSENT, STATE_PRUNED, STATE_READ, STATE_RESTORED};

/// What one host answered about its own periodic table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CronOutcome {
    pub host: String,
    /// One of the `STATE_*` words.
    pub state: String,
    pub detail: String,
    /// The whole table as the host had it, before any change.
    pub table: Vec<String>,
    /// Every non-comment line the pattern reached.
    pub matched: Vec<String>,
    /// Where `--apply` put the table it replaced.
    pub backup_path: Option<String>,
}

impl CronOutcome {
    pub fn changed(&self) -> bool {
        self.state == STATE_PRUNED || self.state == STATE_RESTORED
    }

    pub fn succeeded(&self) -> bool {
        matches!(
            self.state.as_str(),
            STATE_READ | STATE_PRUNED | STATE_ABSENT | STATE_RESTORED
        )
    }

    /// The one command that puts back what `--apply` changed. Printed with
    /// the result rather than left for an operator to compose, because a
    /// reversible action nobody can spell is not reversible.
    pub fn restore_command(&self) -> Option<String> {
        self.backup_path.as_ref().map(|path| {
            format!(
                "stado host cron {} --restore {}",
                self.host,
                shlex_quote(path)
            )
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "state": self.state,
            "detail": self.detail,
            "table": self.table,
            "matched": self.matched,
            "backup_path": self.backup_path,
            "restore_command": self.restore_command(),
        })
    }
}
