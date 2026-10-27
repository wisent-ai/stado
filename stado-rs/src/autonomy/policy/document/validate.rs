//! Refuse an unusable document at the door.
//!
//! The check runs in the order a document is read: the version string, then
//! the limits that must be positive to mean anything, then the freshness
//! TTLs, then the money — every budget must be a finite non-negative number —
//! and finally each resource rule, where a grant that promises something the
//! rule cannot deliver, an invalid cron expression, a pair of identical
//! schedules or an unknown timezone are all rejected.

use std::num::{NonZeroU64, NonZeroUsize};

use super::AutonomyPolicy;

impl AutonomyPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.policy_version.trim().is_empty() {
            return Err("policy_version is required".to_string());
        }
        if NonZeroUsize::new(self.limits.max_actions_per_tick).is_none() {
            return Err("limits.max_actions_per_tick must be positive".to_string());
        }
        if NonZeroUsize::new(self.limits.max_actions_per_provider).is_none() {
            return Err("limits.max_actions_per_provider must be positive".to_string());
        }
        crate::autonomy::time::positive_duration(
            self.limits.decision_ttl_seconds,
            "limits.decision_ttl_seconds",
        )?;
        if NonZeroUsize::new(self.limits.max_concurrent_mutations).is_none() {
            return Err("limits.max_concurrent_mutations must be positive".to_string());
        }
        if NonZeroUsize::new(self.limits.circuit_breaker_failures).is_none() {
            return Err("limits.circuit_breaker_failures must be positive".to_string());
        }
        crate::autonomy::time::positive_duration(
            self.limits.circuit_breaker_cooldown_seconds,
            "limits.circuit_breaker_cooldown_seconds",
        )?;
        if NonZeroU64::new(self.freshness.inventory_max_age_seconds).is_none()
            || NonZeroU64::new(self.freshness.pricing_max_age_seconds).is_none()
        {
            return Err("freshness TTLs must be positive".to_string());
        }
        for (name, amount) in [
            ("hourly_usd", self.budgets.hourly_usd),
            ("daily_usd", self.budgets.daily_usd),
            ("monthly_usd", self.budgets.monthly_usd),
            ("max_single_action_usd", self.budgets.max_single_action_usd),
            ("max_commitment_usd", self.budgets.max_commitment_usd),
            ("local_hourly_cost_usd", self.local_hourly_cost_usd),
        ] {
            if amount.is_some_and(|value| !value.is_finite() || value.is_sign_negative()) {
                return Err(format!("{name} must be a finite non-negative number"));
            }
        }
        for rule in &self.rules {
            let allows_mutation = rule.allow_reversible || rule.allow_destructive;
            if rule.allow_production_mutation && !allows_mutation {
                return Err(
                    "allow_production_mutation requires allow_reversible or allow_destructive"
                        .to_string(),
                );
            }
            if rule.allow_stateful_mutation && !allows_mutation {
                return Err(
                    "allow_stateful_mutation requires allow_reversible or allow_destructive"
                        .to_string(),
                );
            }
            for (name, expression) in [
                ("stop_schedule", rule.stop_schedule.as_deref()),
                ("start_schedule", rule.start_schedule.as_deref()),
            ] {
                if let Some(expression) = expression {
                    if rule.resource_type != Some(crate::autonomy::model::ResourceKind::Instance) {
                        return Err(format!("{name} requires resource_type = instance"));
                    }
                    if !rule.allow_reversible {
                        return Err(format!("{name} requires allow_reversible = true"));
                    }
                    if !crate::schedules::cron_is_valid(expression) {
                        return Err(format!("{name} is not a valid cron expression"));
                    }
                }
            }
            if rule.scale_to_zero
                && (rule.resource_type != Some(crate::autonomy::model::ResourceKind::Instance)
                    || !rule.allow_reversible)
            {
                return Err(
                    "scale_to_zero requires resource_type = instance and allow_reversible = true"
                        .to_string(),
                );
            }
            if rule.stop_schedule.is_some() && rule.stop_schedule == rule.start_schedule {
                return Err("start_schedule and stop_schedule must differ".to_string());
            }
            // A cron expression means nothing without the timezone it is
            // read in: a rule that schedules must name one, and the name
            // must be one the timezone database knows.
            match rule.timezone.as_deref() {
                Some(timezone) => {
                    if timezone.parse::<chrono_tz::Tz>().is_err() {
                        return Err(format!("invalid resource-rule timezone: {timezone}"));
                    }
                }
                None if rule.stop_schedule.is_some() || rule.start_schedule.is_some() => {
                    return Err(
                        "a rule with stop_schedule or start_schedule must name the timezone its \
                         cron expressions are read in (timezone: \"<IANA name>\")"
                            .to_string(),
                    )
                }
                None => {}
            }
        }
        Ok(())
    }
}
