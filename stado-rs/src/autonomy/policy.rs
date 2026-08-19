//! Versioned autonomy policy and fail-closed mutation authorization.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::capabilities::ProviderId;

use super::model::{ResourceRecord, SCHEMA_VERSION};

const TWO: u64 = (u16::BITS / u8::BITS) as u64;
const FIFTEEN: u64 = (u8::BITS as u64 * TWO) - true as u64;
const THIRTY: u64 = u64::BITS as u64 / TWO - TWO;
const DEFAULT_ACTION_LIMIT: usize = (u8::BITS as u64 + TWO) as usize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutonomyMode {
    #[default]
    Report,
    EnforceSafe,
    EnforceOwned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionRisk {
    ReadOnly,
    Reversible,
    Destructive,
    FinancialCommitment,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetPolicy {
    pub hourly_usd: Option<f64>,
    pub daily_usd: Option<f64>,
    pub monthly_usd: Option<f64>,
    pub max_single_action_usd: Option<f64>,
    pub max_commitment_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlacementPolicy {
    pub allowed_providers: BTreeSet<ProviderId>,
    pub allowed_regions: BTreeSet<String>,
    pub prefer_local: bool,
    pub allow_spot: bool,
    pub require_checkpoint_for_spot: bool,
    pub account_for_egress: bool,
}

impl Default for PlacementPolicy {
    fn default() -> Self {
        Self {
            allowed_providers: [
                ProviderId::Local,
                ProviderId::Gcp,
                ProviderId::Azure,
                ProviderId::Aws,
                ProviderId::Box,
                ProviderId::Vast,
            ]
            .into_iter()
            .collect(),
            allowed_regions: BTreeSet::new(),
            prefer_local: true,
            allow_spot: false,
            require_checkpoint_for_spot: true,
            account_for_egress: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdlePolicy {
    pub vm_seconds: u64,
    pub disk_days: u64,
    pub snapshot_days: u64,
    pub artifact_days: u64,
    pub minimum_snapshots: usize,
    pub utilization_window_days: u64,
}

impl Default for IdlePolicy {
    fn default() -> Self {
        Self {
            vm_seconds: crate::monitor::billing::SECONDS_PER_MINUTE * FIFTEEN,
            disk_days: u8::BITS as u64 - true as u64,
            snapshot_days: THIRTY,
            artifact_days: THIRTY,
            minimum_snapshots: true as usize,
            utilization_window_days: u8::BITS as u64 - true as u64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyLimits {
    pub max_actions_per_tick: usize,
    pub max_actions_per_provider: usize,
    pub max_deleted_bytes_per_tick: Option<u64>,
    pub max_concurrent_mutations: usize,
    pub require_complete_inventory: bool,
    pub protect_production: bool,
    pub protect_stateful: bool,
    pub circuit_breaker_failures: usize,
    pub circuit_breaker_cooldown_seconds: u64,
    pub decision_ttl_seconds: u64,
}

impl Default for SafetyLimits {
    fn default() -> Self {
        Self {
            max_actions_per_tick: DEFAULT_ACTION_LIMIT,
            max_actions_per_provider: DEFAULT_ACTION_LIMIT,
            max_deleted_bytes_per_tick: None,
            max_concurrent_mutations: TWO as usize,
            require_complete_inventory: true,
            protect_production: true,
            protect_stateful: true,
            circuit_breaker_failures: (u8::BITS / TWO as u32) as usize,
            circuit_breaker_cooldown_seconds: crate::monitor::billing::SECONDS_PER_MINUTE * FIFTEEN,
            decision_ttl_seconds: crate::monitor::billing::SECONDS_PER_MINUTE
                * (u16::BITS / u8::BITS) as u64
                + crate::monitor::billing::SECONDS_PER_MINUTE * (u8::BITS as u64 / TWO),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceRule {
    pub resource_type: Option<String>,
    pub provider: Option<ProviderId>,
    pub account: Option<String>,
    pub region: Option<String>,
    pub environment: Option<String>,
    pub owner: Option<String>,
    pub policy_ref: String,
    pub allow_reversible: bool,
    pub allow_destructive: bool,
    pub allow_production_mutation: bool,
    pub allow_stateful_mutation: bool,
    pub stop_schedule: Option<String>,
    pub start_schedule: Option<String>,
    pub timezone: Option<String>,
    pub scale_to_zero: bool,
}

impl Default for ResourceRule {
    fn default() -> Self {
        Self {
            resource_type: None,
            provider: None,
            account: None,
            region: None,
            environment: None,
            owner: None,
            policy_ref: "default".to_string(),
            allow_reversible: false,
            allow_destructive: false,
            allow_production_mutation: false,
            allow_stateful_mutation: false,
            stop_schedule: None,
            start_schedule: None,
            timezone: None,
            scale_to_zero: false,
        }
    }
}

impl ResourceRule {
    pub fn matches(&self, resource: &ResourceRecord) -> bool {
        self.resource_type
            .as_deref()
            .is_none_or(|kind| kind == resource.resource_type)
            && self
                .provider
                .is_none_or(|provider| provider == resource.provider)
            && self
                .account
                .as_deref()
                .is_none_or(|account| account == resource.account)
            && self
                .region
                .as_deref()
                .is_none_or(|region| resource.region.as_deref() == Some(region))
            && self
                .environment
                .as_deref()
                .is_none_or(|environment| resource.environment.as_deref() == Some(environment))
            && self
                .owner
                .as_deref()
                .is_none_or(|owner| resource.owner.as_deref() == Some(owner))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FreshnessPolicy {
    pub inventory_max_age_seconds: u64,
    pub pricing_max_age_seconds: u64,
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            inventory_max_age_seconds: crate::monitor::billing::SECONDS_PER_MINUTE
                * (u8::BITS as u64 - (u16::BITS / u8::BITS) as u64 - true as u64),
            pricing_max_age_seconds: crate::monitor::billing::SECONDS_PER_HOUR,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutonomyPolicy {
    pub schema_version: u16,
    pub policy_version: String,
    pub mode: AutonomyMode,
    pub emergency_paused: bool,
    pub budgets: BudgetPolicy,
    pub placement: PlacementPolicy,
    pub idle: IdlePolicy,
    pub freshness: FreshnessPolicy,
    pub limits: SafetyLimits,
    pub local_hourly_cost_usd: Option<f64>,
    pub rules: Vec<ResourceRule>,
    pub metadata: BTreeMap<String, String>,
}

impl Default for AutonomyPolicy {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            policy_version: "default-report-only".to_string(),
            mode: AutonomyMode::Report,
            emergency_paused: false,
            budgets: BudgetPolicy::default(),
            placement: PlacementPolicy::default(),
            idle: IdlePolicy::default(),
            freshness: FreshnessPolicy::default(),
            limits: SafetyLimits::default(),
            local_hourly_cost_usd: None,
            rules: Vec::new(),
            metadata: BTreeMap::new(),
        }
    }
}

impl AutonomyPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported autonomy policy schema_version {}",
                self.schema_version
            ));
        }
        if self.policy_version.trim().is_empty() {
            return Err("policy_version is required".to_string());
        }
        if self.limits.max_actions_per_tick == usize::default() {
            return Err("limits.max_actions_per_tick must be positive".to_string());
        }
        if self.limits.max_actions_per_provider == usize::default() {
            return Err("limits.max_actions_per_provider must be positive".to_string());
        }
        if self.limits.decision_ttl_seconds == u64::default()
            || self.limits.decision_ttl_seconds > i64::MAX as u64
        {
            return Err("limits.decision_ttl_seconds must fit positive i64 seconds".to_string());
        }
        if self.limits.max_concurrent_mutations == usize::default() {
            return Err("limits.max_concurrent_mutations must be positive".to_string());
        }
        if self.limits.circuit_breaker_failures == usize::default()
            || self.limits.circuit_breaker_cooldown_seconds == u64::default()
            || self.limits.circuit_breaker_cooldown_seconds > i64::MAX as u64
        {
            return Err(
                "circuit-breaker threshold and cooldown must fit positive seconds".to_string(),
            );
        }
        if self.freshness.inventory_max_age_seconds == u64::default()
            || self.freshness.pricing_max_age_seconds == u64::default()
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
            if amount.is_some_and(|value| !value.is_finite() || value < f64::default()) {
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
                    if rule.resource_type.as_deref() != Some("instance") {
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
                && (rule.resource_type.as_deref() != Some("instance") || !rule.allow_reversible)
            {
                return Err(
                    "scale_to_zero requires resource_type = instance and allow_reversible = true"
                        .to_string(),
                );
            }
            if rule.stop_schedule.is_some() && rule.stop_schedule == rule.start_schedule {
                return Err("start_schedule and stop_schedule must differ".to_string());
            }
            if let Some(timezone) = rule.timezone.as_deref() {
                if timezone.parse::<chrono_tz::Tz>().is_err() {
                    return Err(format!("invalid resource-rule timezone: {timezone}"));
                }
            }
        }
        Ok(())
    }

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

fn is_stateful(resource: &ResourceRecord) -> bool {
    if matches!(
        resource.resource_type.as_str(),
        "database" | "cloud_sql" | "rds" | "managed_disk" | "persistent_disk" | "volume"
    ) {
        return true;
    }
    if resource.labels.iter().any(|(key, value)| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "stateful" | "stado-stateful" | "stado.io/stateful"
        ) && matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "yes" | "stateful"
        )
    }) {
        return true;
    }
    let gcp_data_disk = resource
        .evidence
        .pointer("/item/disks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|disks| {
            disks.iter().any(|disk| {
                disk.get("boot").and_then(serde_json::Value::as_bool) == Some(false)
                    || disk.get("autoDelete").and_then(serde_json::Value::as_bool) == Some(false)
            })
        });
    let azure_data_disk = resource
        .evidence
        .pointer("/properties/storageProfile/dataDisks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|disks| !disks.is_empty());
    let aws_root = resource
        .evidence
        .get("root_device_name")
        .and_then(serde_json::Value::as_str);
    let aws_data_disk = resource
        .evidence
        .get("block_devices")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|devices| {
            devices.iter().any(|device| {
                device
                    .get("delete_on_termination")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                    || aws_root.is_some_and(|root| {
                        device
                            .get("device_name")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|name| name != root)
                    })
            })
        });
    gcp_data_disk || azure_data_disk || aws_data_disk
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

