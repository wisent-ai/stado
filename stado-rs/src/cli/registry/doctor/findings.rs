//! One way the registry and the live fleet disagree, in the shape every
//! check here produces and the command renders.

use serde_json::{json, Value};

/// One way the registry and the live fleet disagree.
pub(super) struct Finding {
    /// Stable machine-readable category.
    pub(super) kind: &'static str,
    /// Target, host slug or consumer the finding is about.
    pub(super) subject: String,
    /// What specifically disagrees.
    pub(super) detail: String,
    /// Unit label this finding is about, when it is about one.
    ///
    /// Two rows for one cause is noise. A unit that is missing because its host
    /// cannot satisfy what the unit needs produces both a `capability-unsatisfied`
    /// and a `missing-plist`; the second is the symptom of the first, and
    /// [`doctor`](super::doctor) drops it so the row that survives names the cause.
    pub(super) unit: Option<String>,
}

impl Finding {
    pub(super) fn new(
        kind: &'static str,
        subject: impl AsRef<str>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            subject: subject.as_ref().to_string(),
            detail: detail.into(),
            unit: None,
        }
    }

    /// Name the unit this finding is about, so one cause cannot be reported twice.
    pub(super) fn about(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    pub(super) fn to_json(&self) -> Value {
        json!({"finding": self.kind, "subject": self.subject, "detail": self.detail})
    }
}
