//! Every boundary's live verdict, the documents served from it, and what a
//! request is allowed to do about a boundary it found closed.

use std::time::Instant;

use serde_json::{json, Value};

use super::Boundary;

/// One boundary's live verdict.
///
/// This used to be a bare `bool` decided once at startup and never revisited,
/// so a single slow or reset vault read shut a boundary until somebody
/// restarted the unit — and `object` shutting answers `503 object
/// authorization unavailable` to the whole fleet. Recovery is a property of
/// this state now: `attempted_at` is the cooldown anchor an inline
/// revalidation claims before it runs.
#[derive(Clone, Default)]
pub(crate) struct BoundaryVerdict {
    pub(crate) ready: bool,
    /// The monotonic clock of the last validation attempt. Not a wall clock:
    /// a clock step must not be able to skip the cooldown or stretch it past
    /// the next request.
    pub(crate) attempted_at: Option<Instant>,
    /// The validator's own sentence for the last failure, or `None` while the
    /// boundary is open.
    ///
    /// Without this, a closed boundary is one bit and an operator cannot tell
    /// the two answers apart that need opposite responses: `validation did not
    /// settle within N seconds` is arithmetic against the item budget, while
    /// `item set mismatch` or `missing or empty` is a credential answer, and a
    /// credential answer is not fixed by restarting the process. On
    /// 2026-09-03 that distinction was unreachable for a live closed boundary:
    /// `/healthz` publishes booleans by design, the process holding the
    /// verdict logged no boundary line, and the doctor's remedy named
    /// `stado service logs com.wisent.always-on.stado-object-api`, which on
    /// that host answers `no unit file ... in the daemon or agent
    /// directories`. A remedy naming an unreadable artefact is worse than
    /// none.
    pub(crate) last_error: Option<String>,
    /// When that verdict was reached, in wall-clock terms, for the operator
    /// document. `attempted_at` above is monotonic and deliberately so — it
    /// anchors the cooldown and must survive a clock step — but a monotonic
    /// instant means nothing to a reader comparing this against a log.
    pub(crate) checked_at: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct BoundaryAvailability {
    verdicts: [BoundaryVerdict; 8],
}

impl BoundaryAvailability {
    fn verdict(&self, boundary: Boundary) -> &BoundaryVerdict {
        &self.verdicts[boundary as usize]
    }

    pub(crate) fn verdict_mut(&mut self, boundary: Boundary) -> &mut BoundaryVerdict {
        &mut self.verdicts[boundary as usize]
    }

    pub(crate) fn ready(&self, boundary: Boundary) -> bool {
        self.verdict(boundary).ready
    }

    /// The flat booleans `/healthz` has always published. That route answers
    /// before authorization, so it stays booleans: a `last_error` names vault
    /// items, grants and endpoints, and an unauthenticated liveness probe has
    /// no business reading those.
    pub(crate) fn ready_json(&self) -> Value {
        Value::Object(
            Boundary::ALL
                .iter()
                .map(|boundary| (boundary.key().to_string(), json!(self.ready(*boundary))))
                .collect(),
        )
    }

    /// Every boundary with its verdict AND the validator's own sentence, for
    /// `/api/state.json`.
    ///
    /// Separate from [`Self::ready_json`] on purpose: `/healthz` is the
    /// unauthenticated liveness probe and stays booleans, this is the
    /// operator's read. The sentence is the verifier's own words about what
    /// refused — a reason and its subject — never the material itself, which
    /// the verifiers do not put in their error text.
    pub(crate) fn state_json(&self) -> Value {
        Value::Object(
            Boundary::ALL
                .iter()
                .map(|boundary| {
                    let verdict = self.verdict(*boundary);
                    (
                        boundary.key().to_string(),
                        json!({
                            "ready": verdict.ready,
                            "last_error": verdict.last_error,
                            "checked_at": verdict.checked_at,
                            "required_by": boundary.required_by(),
                            "label": boundary.label(),
                        }),
                    )
                })
                .collect(),
        )
    }

    pub(crate) fn all_ready(&self) -> bool {
        Boundary::ALL.iter().all(|boundary| self.ready(*boundary))
    }
}

/// What a request is allowed to do about a boundary it found closed.
pub(crate) enum Recheck {
    /// Already open; proceed.
    Ready,
    /// Closed, and this request owns the one revalidation attempt.
    Claimed,
    /// Closed, and an attempt inside the cooldown already answered for it.
    CoolingDown,
}
