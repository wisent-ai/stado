//! One resource rule: the selector that picks resources, and the grants that
//! selection carries.
//!
//! The optional selector fields are matched conjunctively by `matches` — an
//! absent field matches everything — and the boolean grants below them are
//! the only way a mutation is ever authorized. The schedule expressions and
//! `scale_to_zero` ride along on the same rule because they are reversible
//! mutations of the resources it selects. Every field name here is a
//! published document key.

use serde::{Deserialize, Serialize};

use crate::autonomy::model::ResourceRecord;
use crate::capabilities::ProviderId;

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
