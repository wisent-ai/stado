//! The stored lease document and its fence-gated mutations: every change a
//! live owner may make to its own lease, plus the takeover an expired one
//! cannot refuse.

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{LeaseError, LeaseState};

/// Python `ProviderLease` dataclass. `version` is the backend CAS token:
/// serialized nowhere (Python `field(default="", repr=False, compare=False)`
/// plus `to_dict()` popping it).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderLease {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub fence_token: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub owner_expires_at: String,
    #[serde(default)]
    pub resource_expires_at: String,
    #[serde(default)]
    pub provider_resource_id: String,
    #[serde(default)]
    pub operation_id: String,
    #[serde(default)]
    pub operation_started_at: String,
    #[serde(default)]
    pub prompt_id: String,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub result_state: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(skip)]
    pub version: String,
}

/// Python `datetime.fromisoformat(value.replace("Z", "+00:00"))`.
fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, LeaseError> {
    DateTime::parse_from_rfc3339(&value.replace('Z', "+00:00"))
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| LeaseError::Value(format!("invalid lease timestamp {value:?}: {err}")))
}

/// Python `datetime.now(timezone.utc).isoformat()`.
fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

impl ProviderLease {
    /// Python `ProviderLease.new`.
    pub fn new(
        job_id: &str,
        provider: &str,
        owner_id: &str,
        owner_ttl_seconds: i64,
        resource_ttl_seconds: i64,
    ) -> Self {
        let now = Utc::now();
        let now_iso = now.to_rfc3339();
        ProviderLease {
            job_id: job_id.to_string(),
            provider: provider.to_string(),
            owner_id: owner_id.to_string(),
            fence_token: Uuid::new_v4().simple().to_string(),
            state: LeaseState::Allocating.as_str().to_string(),
            owner_expires_at: (now + Duration::seconds(owner_ttl_seconds)).to_rfc3339(),
            resource_expires_at: (now + Duration::seconds(resource_ttl_seconds)).to_rfc3339(),
            created_at: now_iso.clone(),
            updated_at: now_iso,
            ..Default::default()
        }
    }

    /// Python `to_dict()`: every field except `version`.
    pub(super) fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("lease serialization is infallible")
    }

    /// Python `owner_expired()` (now = current time).
    pub fn owner_expired(&self) -> Result<bool, LeaseError> {
        self.owner_expired_at(Utc::now())
    }

    /// Python `owner_expired(now=...)`.
    pub fn owner_expired_at(&self, now: DateTime<Utc>) -> Result<bool, LeaseError> {
        Ok(now >= parse_timestamp(&self.owner_expires_at)?)
    }

    /// Python `assert_fence`: owner_id + fence_token must match and the
    /// owner TTL must not have lapsed.
    pub fn assert_fence(&self, owner_id: &str, fence_token: &str) -> Result<(), LeaseError> {
        if self.owner_id != owner_id || self.fence_token != fence_token || self.owner_expired()? {
            return Err(LeaseError::conflict(
                "provider lease fence is no longer valid",
            ));
        }
        Ok(())
    }

    /// Python `renew_owner`.
    pub fn renew_owner(
        &mut self,
        owner_id: &str,
        fence_token: &str,
        ttl_seconds: i64,
    ) -> Result<(), LeaseError> {
        self.assert_fence(owner_id, fence_token)?;
        let now = Utc::now();
        self.owner_expires_at = (now + Duration::seconds(ttl_seconds)).to_rfc3339();
        self.updated_at = now.to_rfc3339();
        Ok(())
    }

    /// Python `renew_resource`.
    pub fn renew_resource(
        &mut self,
        owner_id: &str,
        fence_token: &str,
        ttl_seconds: i64,
    ) -> Result<(), LeaseError> {
        self.assert_fence(owner_id, fence_token)?;
        let now = Utc::now();
        self.resource_expires_at = (now + Duration::seconds(ttl_seconds)).to_rfc3339();
        self.updated_at = now.to_rfc3339();
        Ok(())
    }

    /// Python `transition`: fence-gated state-machine step through
    /// `_ALLOWED_TRANSITIONS`.
    pub fn transition(
        &mut self,
        state: LeaseState,
        owner_id: &str,
        fence_token: &str,
    ) -> Result<(), LeaseError> {
        self.assert_fence(owner_id, fence_token)?;
        let current = LeaseState::from_str(&self.state)?;
        if !current.allowed_transitions().contains(&state) {
            return Err(LeaseError::Value(format!(
                "invalid provider lease transition {} -> {}",
                current.as_str(),
                state.as_str()
            )));
        }
        self.state = state.as_str().to_string();
        self.updated_at = now_iso();
        Ok(())
    }

    /// Python `takeover`: re-owner a lease whose owner TTL has lapsed,
    /// rotating the fence token.
    pub fn takeover(&mut self, owner_id: &str, owner_ttl_seconds: i64) -> Result<(), LeaseError> {
        if !self.owner_expired()? {
            return Err(LeaseError::conflict("provider lease owner is still live"));
        }
        let now = Utc::now();
        self.owner_id = owner_id.to_string();
        self.fence_token = Uuid::new_v4().simple().to_string();
        self.owner_expires_at = (now + Duration::seconds(owner_ttl_seconds)).to_rfc3339();
        self.updated_at = now.to_rfc3339();
        Ok(())
    }

    /// Python `relinquish`: fence-gated immediate owner expiry.
    pub fn relinquish(&mut self, owner_id: &str, fence_token: &str) -> Result<(), LeaseError> {
        self.assert_fence(owner_id, fence_token)?;
        let now = now_iso();
        self.owner_expires_at = now.clone();
        self.updated_at = now;
        Ok(())
    }
}
