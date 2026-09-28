//! The one mutation gate every service repair of a pass goes through.
//!
//! A repair runs only inside the pass's action limit, while the live control
//! document shows no emergency pause and no open circuit breaker, and while
//! this pass holds the placement lease of the exact unit it changes. The lease
//! is released after the repair, and a lease that changed hands in between
//! turns the repair's result into a failure. Report mode and the policy's own
//! pause are checked by the caller, which records them as `planned`.

use chrono::Utc;

use crate::autonomy::policy::AutonomyPolicy;
use crate::autonomy::storage::PlacementLease;
use crate::queue::{JobStorage, StorageError};

/// The classification a lease held elsewhere records, named outside the
/// `else` branch that stores it, as the write gate requires.
const LEASE_BLOCKED: &str = "lease_blocked";

/// Why a mutation was not admitted: the outcome's classification and detail.
pub(super) struct Refusal {
    pub(super) classification: &'static str,
    pub(super) detail: String,
}

pub(super) struct MutationGate<'a> {
    store: &'a JobStorage,
    policy: &'a AutonomyPolicy,
    decision_id: &'a str,
    mutations: usize,
}

impl<'a> MutationGate<'a> {
    pub(super) fn new(
        store: &'a JobStorage,
        policy: &'a AutonomyPolicy,
        decision_id: &'a str,
    ) -> Self {
        Self {
            store,
            policy,
            decision_id,
            mutations: usize::default(),
        }
    }

    /// Admit one mutation of `host`'s `unit`: the action limit, the live
    /// control document, then the unit's lease. Counts the mutation when the
    /// lease is taken.
    pub(super) async fn admit(
        &mut self,
        host: &str,
        unit: &str,
    ) -> Result<Result<(String, PlacementLease), Refusal>, StorageError> {
        if self.mutations >= self.policy.limits.max_actions_per_tick {
            return Ok(Err(Refusal {
                classification: "action_limit",
                detail: "service action limit reached for this autonomy tick".to_string(),
            }));
        }
        let control = crate::autonomy::storage::load_control(self.store).await?;
        if control.emergency_paused || control.circuit_open_at(Utc::now()) {
            return Ok(Err(Refusal {
                classification: "control_blocked",
                detail: "autonomy pause or circuit breaker became active".to_string(),
            }));
        }
        let subject = format!("service:{host}:{unit}");
        let Some(lease) = crate::autonomy::storage::acquire_placement_lease(
            self.store,
            &subject,
            self.decision_id,
            "service-reconciler",
            self.policy.limits.decision_ttl_seconds,
            Utc::now(),
        )
        .await?
        else {
            return Ok(Err(Refusal {
                classification: LEASE_BLOCKED,
                detail: "another reconciler owns this service mutation".to_string(),
            }));
        };
        self.mutations += 1;
        Ok(Ok((subject, lease)))
    }

    /// Release the lease `admit` took. A lease that changed hands, or a
    /// release that failed, turns the repair's result into an error.
    pub(super) async fn release<T>(
        &self,
        subject: &str,
        lease: &PlacementLease,
        result: Result<T, String>,
    ) -> Result<T, String> {
        match crate::autonomy::storage::release_placement_lease(self.store, subject, &lease.token)
            .await
        {
            Ok(true) => result,
            Ok(false) => Err(
                "service action finished, but mutation lease ownership changed before release"
                    .to_string(),
            ),
            Err(error) => Err(format!(
                "service action finished, but mutation lease release failed: {error}"
            )),
        }
    }

    /// Feed one host mutation's result to the circuit breaker.
    pub(super) async fn record(&self, error: Option<&str>) -> Result<(), StorageError> {
        crate::autonomy::storage::record_mutation_outcome(
            self.store,
            error.is_none(),
            error,
            self.policy.limits.circuit_breaker_failures,
            self.policy.limits.circuit_breaker_cooldown_seconds,
        )
        .await
    }
}
