//! The authorization boundaries this listener gates its routes on: the
//! vocabulary ([`kind`]), every boundary's live verdict ([`state`]), the
//! validation budget ([`budget`]), the per-request plan ([`plan`]), and the
//! validate/record/recover sequence one request runs against them.

mod budget;
mod kind;
mod plan;
mod state;

use std::time::Instant;

use crate::dashboard::integration;
use crate::rate_limit;

use super::Dashboard;
use budget::{boundary_recheck_cooldown, boundary_timeout};
use state::{BoundaryVerdict, Recheck};

pub use budget::BOUNDARY_TIMEOUT_OVERRIDE_PATH;

pub(crate) use kind::Boundary;
pub(crate) use plan::{boundary_plan, requires_object_boundary, BoundaryPlan};
pub(crate) use state::BoundaryAvailability;

impl Dashboard {
    /// Run exactly one boundary's verifier once, bounded by
    /// [`boundary_timeout`], and flatten every failure shape — refusal,
    /// timeout, misconfiguration — into the one sentence an operator reads in
    /// the log and in `last_error`.
    pub(crate) async fn validate_boundary(&self, boundary: Boundary) -> Result<(), String> {
        let timeout = boundary_timeout(boundary);
        macro_rules! bounded {
            ($call:expr) => {
                match tokio::time::timeout(timeout, $call).await {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err(format!(
                        "validation did not settle within {} seconds, reading one vault field \
                         per mapped item serially; a vault that accepts connections without \
                         answering inside that budget looks identical to a missing grant here",
                        timeout.as_secs()
                    )),
                }
            };
        }
        match boundary {
            Boundary::Object => bounded!(crate::skarbiec::validate_object_verifier()),
            Boundary::Release => bounded!(crate::skarbiec::validate_release_verifier()),
            Boundary::Machine => bounded!(crate::skarbiec::validate_machine_verifier()),
            Boundary::Service => bounded!(crate::skarbiec::validate_service_verifier()),
            Boundary::RateLimitVerifier => bounded!(rate_limit::validate_verifier()),
            Boundary::RateLimitState => bounded!(self.rate_limiter.restore()),
            Boundary::Integration => bounded!(integration::validate_startup()),
            Boundary::Registry => bounded!(crate::skarbiec::validate_registry_verifier()),
        }
    }

    /// Record one validation outcome as this boundary's current verdict.
    pub(crate) fn record_boundary(&self, boundary: Boundary, outcome: Result<(), String>) {
        let verdict = BoundaryVerdict {
            ready: outcome.is_ok(),
            attempted_at: Some(Instant::now()),
            last_error: outcome.as_ref().err().cloned(),
            checked_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        *self
            .boundaries
            .write()
            .expect("dashboard boundary state lock")
            .verdict_mut(boundary) = verdict;
    }

    /// Whether `boundary` is open right now. Never touches the vault, so this
    /// is what every request on a healthy listener pays: one read lock.
    fn boundary_ready(&self, boundary: Boundary) -> bool {
        self.boundaries
            .read()
            .expect("dashboard boundary state lock")
            .ready(boundary)
    }

    /// Decide what this request may do about `boundary`, and — when it may
    /// revalidate — claim the attempt by stamping `attempted_at` before the
    /// vault is touched. Claiming under the write lock is what keeps a fleet
    /// hammering a shut boundary to one vault sweep per cooldown instead of
    /// one per request.
    fn claim_boundary_recheck(&self, boundary: Boundary) -> Recheck {
        if self.boundary_ready(boundary) {
            return Recheck::Ready;
        }
        let now = Instant::now();
        let cooldown = boundary_recheck_cooldown();
        let mut boundaries = self
            .boundaries
            .write()
            .expect("dashboard boundary state lock");
        let verdict = boundaries.verdict_mut(boundary);
        if verdict.ready {
            return Recheck::Ready;
        }
        if verdict
            .attempted_at
            .is_some_and(|attempted_at| now.duration_since(attempted_at) < cooldown)
        {
            return Recheck::CoolingDown;
        }
        verdict.attempted_at = Some(now);
        Recheck::Claimed
    }

    /// Ready-or-recover for one boundary: revalidate a closed boundary inline,
    /// at most once per cooldown, and answer whether the request may proceed.
    ///
    /// This is the recovery half of the startup sweep. Before it, a boundary
    /// closed by one slow or reset read stayed closed until a privileged unit
    /// restart — and for `com.wisent.always-on.stado-object-api` that restart
    /// is exactly the thing the fleet cannot do for itself.
    async fn recover_boundary(&self, boundary: Boundary) -> bool {
        match self.claim_boundary_recheck(boundary) {
            Recheck::Ready => return true,
            Recheck::CoolingDown => return false,
            Recheck::Claimed => {}
        }
        eprintln!(
            "[dashboard] {} boundary is closed; revalidating inline (required by {})",
            boundary.label(),
            boundary.required_by()
        );
        let outcome = self.validate_boundary(boundary).await;
        match &outcome {
            Ok(()) => eprintln!(
                "[dashboard] {} boundary recovered without a restart",
                boundary.label()
            ),
            Err(error) => eprintln!(
                "[dashboard] {} boundary revalidation failed: {error}",
                boundary.label()
            ),
        }
        let ready = outcome.is_ok();
        self.record_boundary(boundary, outcome);
        ready
    }

    /// Whether every boundary a route needs is open, revalidating at most one
    /// closed boundary. One per request on purpose: a single request must not
    /// be able to turn into a fan of vault sweeps, and the first closed
    /// boundary is the one the refusal already names.
    pub(crate) async fn boundaries_available(&self, required: &[Boundary]) -> bool {
        let mut attempted = false;
        for &boundary in required {
            if self.boundary_ready(boundary) {
                continue;
            }
            if attempted {
                return false;
            }
            attempted = true;
            if !self.recover_boundary(boundary).await {
                return false;
            }
        }
        true
    }

    /// Carry out one request's [`BoundaryPlan`]: revalidate what it asks
    /// about, then answer whether what it enforces is open.
    ///
    /// The two halves are deliberately different sets. `boundaries_available`
    /// fuses them — it revalidates exactly what it gates on — and that fusion
    /// is what let `Boundary::Release` become its own precondition. Here a
    /// request can ask about a boundary it is not gated by, which is the only
    /// way a boundary excluded from its own gate ever reopens.
    ///
    /// Still one vault sweep per request: a request must not turn into a fan
    /// of serial gpg decryptions, and the cooldown in
    /// [`Self::claim_boundary_recheck`] keeps a fleet hammering a shut
    /// boundary to one attempt per window.
    pub(crate) async fn satisfy_boundaries(&self, plan: &BoundaryPlan) -> bool {
        let mut attempted = false;
        for &boundary in &plan.revalidated {
            if self.boundary_ready(boundary) {
                continue;
            }
            if attempted {
                break;
            }
            attempted = true;
            self.recover_boundary(boundary).await;
        }
        plan.enforced
            .iter()
            .all(|&boundary| self.boundary_ready(boundary))
    }
}
