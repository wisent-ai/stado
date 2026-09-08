//! One preflight result and the constructors every probe builds it with.

use serde_json::{json, Value};

use super::status::Status;

/// One preflight result. `remedy` is populated on every check, including
/// passing ones, so `--fix-hints` can print the knob that governs a check
/// which currently passes; the default rendering shows it only for WARN
/// and FAIL rows, where it is the actionable part.
#[derive(Debug, Clone)]
pub struct Check {
    /// Stable machine-readable id (`config`, `storage`, ...).
    pub id: &'static str,
    /// Human column heading.
    pub title: &'static str,
    pub status: Status,
    /// What was observed. Carries the EXACT upstream error text on
    /// failure — never a paraphrase, never just "failed".
    pub detail: String,
    /// The env var or command that changes the outcome.
    pub remedy: String,
    /// What this check interrogated, as fields rather than as prose.
    ///
    /// Empty means the check counts no subjects — one endpoint, one config
    /// file, one yes-or-no question — not that it looked at nothing. A check
    /// that does count subjects puts the number here as well as in `detail`,
    /// so "did this check actually measure anything" is answerable from
    /// `doctor --json` without parsing English.
    pub measured: Vec<crate::fleet_shape::Measurement>,
}

impl Check {
    pub(in crate::doctor) fn new(
        id: &'static str,
        title: &'static str,
        status: Status,
        detail: String,
        remedy: &str,
    ) -> Self {
        Self {
            id,
            title,
            status,
            detail,
            remedy: remedy.to_string(),
            measured: Vec::new(),
        }
    }

    pub(in crate::doctor) fn pass(
        id: &'static str,
        title: &'static str,
        detail: String,
        remedy: &str,
    ) -> Self {
        Self::new(id, title, Status::Pass, detail, remedy)
    }

    pub(in crate::doctor) fn fail(
        id: &'static str,
        title: &'static str,
        detail: String,
        remedy: &str,
    ) -> Self {
        Self::new(id, title, Status::Fail, detail, remedy)
    }

    /// A check whose probe never answered. Carries no verdict about the
    /// deployment, and says which budget it ran out of.
    pub(in crate::doctor) fn unmeasured(
        id: &'static str,
        title: &'static str,
        detail: String,
        remedy: &str,
    ) -> Self {
        Self::new(id, title, Status::Unmeasured, detail, remedy)
    }

    /// Record what one rule interrogated. Chainable so a probe can state its
    /// subject count on the same expression that builds the row.
    pub(in crate::doctor) fn measuring(
        mut self,
        check: &'static str,
        host: Option<String>,
        subjects: u64,
    ) -> Self {
        self.measured
            .push(crate::fleet_shape::Measurement::new(check, host, subjects));
        self
    }

    pub(super) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "status": self.status.key(),
            "detail": self.detail,
            "remedy": self.remedy,
            "measured": self.measured.iter().map(crate::fleet_shape::Measurement::to_json).collect::<Vec<Value>>(),
        })
    }
}
