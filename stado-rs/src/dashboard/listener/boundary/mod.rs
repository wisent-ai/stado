//! The authorization boundaries this listener gates its routes on: the
//! vocabulary ([`kind`]), every boundary's live verdict ([`state`]), the
//! per-request plan ([`plan`]), and the validate/record/recover sequence one
//! request runs against them.

mod kind;
mod plan;
mod state;

use crate::dashboard::integration;
use crate::rate_limit;

use super::Dashboard;
use state::{BoundaryVerdict, Recheck};

pub(crate) use kind::Boundary;
pub(crate) use plan::{boundary_plan, requires_object_boundary, BoundaryPlan};
pub(crate) use state::BoundaryAvailability;

impl Dashboard {
    /// Run exactly one boundary's verifier once, until it answers, and
    /// flatten every failure shape — refusal, misconfiguration — into the one
    /// sentence an operator reads in the log and in `last_error`.
    pub(crate) async fn validate_boundary(&self, boundary: Boundary) -> Result<(), String> {
        fn flat<T, E: std::fmt::Display>(outcome: Result<T, E>) -> Result<(), String> {
            outcome.map(|_| ()).map_err(|error| error.to_string())
        }
        match boundary {
            Boundary::Object => flat(crate::skarbiec::validate_object_verifier().await),
            Boundary::Release => flat(crate::skarbiec::validate_release_verifier().await),
            Boundary::Machine => flat(crate::skarbiec::validate_machine_verifier().await),
            Boundary::Service => flat(crate::skarbiec::validate_service_verifier().await),
            Boundary::RateLimitVerifier => flat(rate_limit::validate_verifier().await),
            Boundary::RateLimitState => flat(self.rate_limiter.restore().await),
            Boundary::Integration => flat(integration::validate_startup().await),
            Boundary::Registry => flat(crate::skarbiec::validate_registry_verifier().await),
        }
    }

    /// Record one validation outcome as this boundary's current verdict.
    pub(crate) fn record_boundary(&self, boundary: Boundary, outcome: Result<(), String>) {
        let verdict = BoundaryVerdict {
            ready: outcome.is_ok(),
            recheck_in_flight: false,
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
    /// revalidate — claim the attempt under the write lock before the vault
    /// is touched, so one sweep runs at a time however many requests find the
    /// boundary shut.
    fn claim_boundary_recheck(&self, boundary: Boundary) -> Recheck {
        if self.boundary_ready(boundary) {
            return Recheck::Ready;
        }
        let mut boundaries = self
            .boundaries
            .write()
            .expect("dashboard boundary state lock");
        let verdict = boundaries.verdict_mut(boundary);
        if verdict.ready {
            return Recheck::Ready;
        }
        if verdict.recheck_in_flight {
            return Recheck::InFlight;
        }
        verdict.recheck_in_flight = true;
        Recheck::Claimed
    }

    /// Release a claim whose request ended before it recorded an outcome —
    /// the client went away mid-sweep — so the next request can revalidate.
    fn release_boundary_recheck(&self, boundary: Boundary) {
        self.boundaries
            .write()
            .expect("dashboard boundary state lock")
            .verdict_mut(boundary)
            .recheck_in_flight = false;
    }

    /// Ready-or-recover for one boundary: revalidate a closed boundary inline
    /// unless another request already is, and answer whether the request may
    /// proceed.
    ///
    /// This is the recovery half of the startup sweep. Before it, a boundary
    /// closed by one slow or reset read stayed closed until a privileged unit
    /// restart — and for the host Stado process that restart is exactly the
    /// thing the fleet cannot do for itself.
    async fn recover_boundary(&self, boundary: Boundary) -> bool {
        match self.claim_boundary_recheck(boundary) {
            Recheck::Ready => return true,
            Recheck::InFlight => return false,
            Recheck::Claimed => {}
        }
        // Holds the claim until an outcome is recorded; dropped early only
        // when this request's future is, and then it frees the claim.
        struct Claim<'a> {
            dashboard: &'a Dashboard,
            boundary: Boundary,
            settled: bool,
        }
        impl Drop for Claim<'_> {
            fn drop(&mut self) {
                if !self.settled {
                    self.dashboard.release_boundary_recheck(self.boundary);
                }
            }
        }
        let mut claim = Claim {
            dashboard: self,
            boundary,
            settled: false,
        };
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
        claim.settled = true;
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
    /// of serial gpg decryptions, and the claim in
    /// [`Self::claim_boundary_recheck`] keeps a fleet hammering a shut
    /// boundary to one sweep at a time.
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
