//! Every boundary's live verdict, the documents served from it, and what a
//! request is allowed to do about a boundary it found closed.

use serde_json::{json, Value};

use super::Boundary;

/// One boundary's live verdict.
///
/// This used to be a bare `bool` decided once at startup and never revisited,
/// so a single slow or reset vault read shut a boundary until somebody
/// restarted the unit — and `object` shutting answers `503 object
/// authorization unavailable` to the whole fleet. Recovery is a property of
/// this state now: a request that finds the boundary closed revalidates it
/// inline unless another request's revalidation is already running.
#[derive(Clone, Default)]
pub(crate) struct BoundaryVerdict {
    pub(crate) ready: bool,
    /// A request is revalidating this boundary right now. One sweep runs at a
    /// time, so a fleet hammering a shut boundary waits on that sweep's
    /// answer instead of starting one per request.
    pub(crate) recheck_in_flight: bool,
    /// The validator's own sentence for the last failure, or `None` while the
    /// boundary is open.
    ///
    /// Without this, a closed boundary is one bit and an operator cannot tell
    /// the two answers apart that need opposite responses: `validation did not
    /// settle within N seconds` is arithmetic against the item budget, while
    /// `item set mismatch` or `missing or empty` is a credential answer, and a
    /// credential answer is not fixed by restarting the process. That
    /// distinction was unreachable for a live closed boundary: `/healthz`
    /// publishes booleans by design, the process holding the verdict logged
    /// no boundary line, and the doctor's remedy named a `stado service logs`
    /// unit that host did not have a unit file for. A remedy naming an
    /// unreadable artefact is worse than none.
    pub(crate) last_error: Option<String>,
    /// When that verdict was reached, in wall-clock terms, for the operator
    /// document.
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

    /// Why a request gated on `boundaries` is refused: the first closed one
    /// by its label, its verifier's own last sentence and when that verdict
    /// was reached, or that a recheck is running now. `None` when every one
    /// is open.
    pub(crate) fn closed_cause(&self, boundaries: &[Boundary]) -> Option<String> {
        let boundary = boundaries.iter().find(|boundary| !self.ready(**boundary))?;
        let verdict = self.verdict(*boundary);
        let mut cause = format!("the {} boundary is closed", boundary.label());
        if let Some(error) = &verdict.last_error {
            cause.push_str(": ");
            cause.push_str(error);
        }
        if let Some(checked_at) = &verdict.checked_at {
            cause.push_str(&format!(" (checked {checked_at})"));
        }
        if verdict.recheck_in_flight {
            cause.push_str("; another request is revalidating it now");
        }
        Some(cause)
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
    /// Closed, and another request's revalidation is running.
    InFlight,
}
