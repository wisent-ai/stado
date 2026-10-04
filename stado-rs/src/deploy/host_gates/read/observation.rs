//! Read-only diagnostic I/O, retaining which source did not answer and why.
use std::future::Future;
use std::time::Instant;

use chrono::Utc;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadState {
    Complete,
    Absent,
    Cached,
    Error,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagnosticRead {
    pub operation: &'static str,
    pub source: String,
    pub state: ReadState,
    pub started_at: Option<String>,
    pub finished_at: String,
    pub elapsed_ms: u128,
    pub detail: Option<String>,
}

impl DiagnosticRead {
    pub fn complete(&self) -> bool {
        matches!(self.state, ReadState::Complete | ReadState::Absent)
    }

    pub fn skipped(operation: &'static str, source: String, reason: &str) -> Self {
        Self {
            operation,
            source,
            state: ReadState::Skipped,
            started_at: None,
            finished_at: Utc::now().to_rfc3339(),
            elapsed_ms: 0,
            detail: Some(reason.to_string()),
        }
    }
}

pub(crate) async fn observe<T, E: std::fmt::Display>(
    operation: &'static str,
    source: String,
    read: impl Future<Output = Result<T, E>>,
) -> (Option<T>, DiagnosticRead) {
    let started_at = Utc::now().to_rfc3339();
    let started = Instant::now();
    let (value, state, detail) = match read.await {
        Ok(value) => (Some(value), ReadState::Complete, None),
        Err(error) => (None, ReadState::Error, Some(error.to_string())),
    };
    (
        value,
        DiagnosticRead {
            operation,
            source,
            state,
            started_at: Some(started_at),
            finished_at: Utc::now().to_rfc3339(),
            elapsed_ms: started.elapsed().as_millis(),
            detail,
        },
    )
}
