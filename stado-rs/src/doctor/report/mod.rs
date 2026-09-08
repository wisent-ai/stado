//! Reporting and verdicts: what one check says, what a whole run says, and
//! which probes a run selects.

use serde_json::{json, Value};

use crate::doctor::fleet::hosts::placement::PLACEMENT_ID;
use crate::doctor::fleet::hosts::shape::SHAPE_ID;
use crate::doctor::fleet::releases::channel::RELEASE_ID;
use crate::doctor::fleet::releases::integrity::INTEGRITY_ID;

mod check;
mod findings;
mod status;

pub use check::Check;
pub use status::Status;

pub(in crate::doctor) use findings::Findings;

/// The full preflight outcome, in the order the checks are meant to be
/// read: earliest blocking failure first.
#[derive(Debug, Clone)]
pub struct Report {
    pub generated_at: String,
    pub checks: Vec<Check>,
}

impl Report {
    /// Worst verdict across every check.
    pub fn status(&self) -> Status {
        self.checks
            .iter()
            .fold(Status::Pass, |so_far, check| so_far.worst(check.status))
    }

    /// The first FAIL in preflight order — the one to fix before reading
    /// further, because the later checks are usually downstream of it.
    pub fn first_failure(&self) -> Option<&Check> {
        self.checks
            .iter()
            .find(|check| check.status == Status::Fail)
    }

    pub fn failed(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == Status::Fail)
            .count()
    }

    /// Checks whose probe never answered. Counted separately from `failed`
    /// on purpose: adding them together is what made `doctor` report six
    /// failures when two were real.
    pub fn unmeasured(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == Status::Unmeasured)
            .count()
    }

    pub fn warned(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == Status::Warn)
            .count()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "generated_at": self.generated_at,
            "status": self.status().key(),
            "failed": self.failed(),
            "warned": self.warned(),
            "unmeasured": self.unmeasured(),
            "checks": self.checks.iter().map(Check::to_json).collect::<Vec<Value>>(),
        })
    }
}

/// Which probes `stado doctor` executes. Reduced modes select work before
/// futures are polled; filtering a completed full report would still run the
/// unrelated storage and fleet sweeps and could make the selected check fail
/// under load from work its caller did not request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunScope {
    #[default]
    Full,
    DeploymentPreflight,
    ReleaseVerification,
}

impl RunScope {
    pub(in crate::doctor) fn includes(self, id: &str) -> bool {
        match self {
            Self::Full => true,
            Self::DeploymentPreflight => {
                id != RELEASE_ID && id != INTEGRITY_ID && id != PLACEMENT_ID && id != SHAPE_ID
            }
            Self::ReleaseVerification => id == RELEASE_ID,
        }
    }
}
