//! Per-item outcomes a probe accumulates before it reaches one verdict.

use super::check::Check;
use super::status::Status;

/// Per-item outcomes accumulated inside one check that inspects several
/// things (one row per configured provider, say). The check's verdict is
/// the worst of them, so a healthy GCP arm never masks a broken Azure one.
#[derive(Default)]
pub(in crate::doctor) struct Findings {
    status: Status,
    pub(in crate::doctor) notes: Vec<String>,
    remedies: Vec<String>,
    measurements: Vec<crate::fleet_shape::Measurement>,
}

impl Findings {
    pub(in crate::doctor) fn note(&mut self, status: Status, note: String) {
        self.status = self.status.worst(status);
        self.notes.push(note);
    }

    /// Record the fix for a specific finding, de-duplicated: several
    /// providers failing the same way should not repeat the same remedy.
    pub(in crate::doctor) fn remedy(&mut self, remedy: impl Into<String>) {
        let remedy = remedy.into();
        if !self.remedies.contains(&remedy) {
            self.remedies.push(remedy);
        }
    }

    /// Record what one rule interrogated, beside the prose note that says the
    /// same thing in a sentence.
    pub(in crate::doctor) fn measure(&mut self, measurement: crate::fleet_shape::Measurement) {
        self.measurements.push(measurement);
    }

    /// `base` is the knob that governs this check when nothing went wrong.
    pub(in crate::doctor) fn into_check(
        self,
        id: &'static str,
        title: &'static str,
        base: &str,
    ) -> Check {
        let remedy = if self.remedies.is_empty() {
            base.to_string()
        } else {
            self.remedies.join(" | ")
        };
        Check {
            id,
            title,
            status: self.status,
            detail: self.notes.join("; "),
            remedy,
            measured: self.measurements,
        }
    }
}
