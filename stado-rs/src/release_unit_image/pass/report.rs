//! The one line one tick writes about itself.

use crate::release_unit_image::ledger::identity::AttemptOutcome;
use crate::release_unit_image::plan::{RevisitPick, RevisitSkip};

/// One tick's account of itself, in `registry doctor`'s kinds and #344's
/// outcome words. No new severity vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisitReport {
    pub host: String,
    pub acted: Option<(RevisitPick, AttemptOutcome)>,
    pub skipped: Vec<(String, RevisitSkip)>,
    /// Set when another process held the host lock, so the tick did nothing.
    pub busy: bool,
}

impl RevisitReport {
    /// The one line the agent writes per tick.
    pub(crate) fn line(&self) -> String {
        if self.busy {
            return format!(
                "stado release agent unit-image revisit host={} skipped: another reconcile holds \
                 the host revisit lock, so no unit was observed or restarted",
                self.host
            );
        }
        let acted = self.acted.as_ref().map_or_else(
            || "unit=- outcome=none".to_string(),
            |(pick, outcome)| {
                format!(
                    "unit={} product={} kind={} outcome={} running={} declared={}",
                    pick.unit,
                    pick.product,
                    pick.kind,
                    outcome.word(),
                    pick.running.describe(),
                    pick.declared.describe()
                )
            },
        );
        let skipped = self
            .skipped
            .iter()
            .map(|(unit, skip)| format!("{unit}: {}", skip.sentence()))
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "stado release agent unit-image revisit host={} {acted} left={}",
            self.host,
            if skipped.is_empty() {
                "-".to_string()
            } else {
                skipped
            }
        )
    }
}
