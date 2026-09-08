//! Fail-closed authorization: the gate every mutation passes through.
//!
//! A read-only action is always allowed. Everything else must survive the
//! whole ladder — the emergency pause, report-only mode, an incomplete
//! inventory, unowned resources, the production and stateful protections and
//! the single-action budget ceiling — before the risk class is finally
//! matched against the mode and the rule that selected the resource. Every
//! rung that does not allow denies, and the verdict carries the reason.

use serde::{Deserialize, Serialize};

use crate::autonomy::model::ResourceRecord;
use crate::autonomy::policy::{ActionRisk, AutonomyMode, ResourceRule};

use super::stateful::is_stateful;
use super::AutonomyPolicy;

impl AutonomyPolicy {
    pub fn matching_rule<'a>(&'a self, resource: &ResourceRecord) -> Option<&'a ResourceRule> {
        self.rules.iter().find(|rule| rule.matches(resource))
    }

    pub fn authorize(
        &self,
        resource: &ResourceRecord,
        risk: ActionRisk,
        inventory_complete: bool,
        estimated_cost_usd: Option<f64>,
    ) -> AuthorizationDecision {
        if risk == ActionRisk::ReadOnly {
            return AuthorizationDecision::allow("read-only action");
        }
        if self.emergency_paused {
            return AuthorizationDecision::deny("autonomy is emergency-paused");
        }
        if self.mode == AutonomyMode::Report {
            return AuthorizationDecision::deny("policy is report-only");
        }
        if self.limits.require_complete_inventory && !inventory_complete {
            return AuthorizationDecision::deny("inventory is incomplete");
        }
        if !resource.ownership.is_mutable() {
            return AuthorizationDecision::deny("resource is not owned or adopted");
        }
        let rule = self.matching_rule(resource);
        if self.limits.protect_production
            && resource.environment.as_deref() == Some("production")
            && !rule.is_some_and(|candidate| candidate.allow_production_mutation)
        {
            return AuthorizationDecision::deny("production resource is protected");
        }
        if self.limits.protect_stateful
            && is_stateful(resource)
            && !rule.is_some_and(|candidate| candidate.allow_stateful_mutation)
        {
            return AuthorizationDecision::deny("stateful resource is protected");
        }
        if estimated_cost_usd
            .zip(self.budgets.max_single_action_usd)
            .is_some_and(|(estimated, maximum)| estimated > maximum)
        {
            return AuthorizationDecision::deny("action exceeds max_single_action_usd");
        }
        match risk {
            ActionRisk::ReadOnly => AuthorizationDecision::allow("read-only action"),
            ActionRisk::Reversible => {
                if self.mode == AutonomyMode::EnforceSafe
                    || rule.is_some_and(|candidate| candidate.allow_reversible)
                {
                    AuthorizationDecision::allow("reversible action allowed by policy")
                } else {
                    AuthorizationDecision::deny("reversible action is not authorized")
                }
            }
            ActionRisk::Destructive => {
                if self.mode == AutonomyMode::EnforceOwned
                    && rule.is_some_and(|candidate| candidate.allow_destructive)
                {
                    AuthorizationDecision::allow("destructive action explicitly allowed")
                } else {
                    AuthorizationDecision::deny("destructive action requires an explicit rule")
                }
            }
            ActionRisk::FinancialCommitment => AuthorizationDecision::deny(
                "financial commitments require an operator-approved immutable plan",
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationDecision {
    pub allowed: bool,
    pub reason: String,
}

impl AuthorizationDecision {
    fn allow(reason: impl Into<String>) -> Self {
        Self {
            allowed: true,
            reason: reason.into(),
        }
    }

    fn deny(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
        }
    }
}
