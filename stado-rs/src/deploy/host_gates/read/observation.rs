//! Bound read-only diagnostic I/O and retain which source did not answer.
use std::future::Future;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;

pub const READ_BUDGET: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadState {
    Complete,
    Absent,
    Cached,
    Error,
    TimedOut,
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
    pub budget_ms: u128,
    pub detail: Option<String>,
}

impl DiagnosticRead {
    pub fn complete(&self) -> bool {
        matches!(self.state, ReadState::Complete | ReadState::Absent)
    }

    pub fn skipped(operation: &'static str, source: String, reason: &str) -> Self {
        Self {
            operation, source, state: ReadState::Skipped, started_at: None,
            finished_at: Utc::now().to_rfc3339(), elapsed_ms: 0,
            budget_ms: READ_BUDGET.as_millis(), detail: Some(reason.to_string()),
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
    let (value, state, detail) = match tokio::time::timeout(READ_BUDGET, read).await {
        Ok(Ok(value)) => (Some(value), ReadState::Complete, None),
        Ok(Err(error)) => (None, ReadState::Error, Some(error.to_string())),
        Err(_) => (None, ReadState::TimedOut, Some(format!(
            "{operation} did not finish reading {source} within {} seconds; the source's state is unknown",
            READ_BUDGET.as_secs()
        ))),
    };
    (value, DiagnosticRead {
        operation, source, state, started_at: Some(started_at),
        finished_at: Utc::now().to_rfc3339(), elapsed_ms: started.elapsed().as_millis(),
        budget_ms: READ_BUDGET.as_millis(), detail,
    })
}
