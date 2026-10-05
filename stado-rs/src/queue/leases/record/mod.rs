//! The stored lease document and its fence-gated mutations: every change a
//! live owner may make to its own lease, plus the takeover a gone one
//! cannot refuse.
//!
//! An owner is a process: the lease records the host and pid that hold it,
//! and the owner is live exactly while that process is (and, inside the
//! holding process, while the invocation that took it has not ended). No
//! clock decides it. A lease written by an older Stado states an
//! `owner_expires_at` instead and is held to it.

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{LeaseError, LeaseState};

mod owner;

pub use owner::{OwnerInvocation, OwnerState, ProcessOwner};

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
    /// The host whose process holds the lease; empty once released.
    #[serde(default)]
    pub owner_host: String,
    /// The pid holding the lease on `owner_host`; 0 once released.
    #[serde(default)]
    pub owner_pid: u32,
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
    /// Python `ProviderLease.new`, held by this process.
    pub fn new(job_id: &str, provider: &str, owner_id: &str, resource_ttl_seconds: i64) -> Self {
        let now = Utc::now();
        let now_iso = now.to_rfc3339();
        let holder = ProcessOwner::current();
        ProviderLease {
            job_id: job_id.to_string(),
            provider: provider.to_string(),
            owner_id: owner_id.to_string(),
            fence_token: Uuid::new_v4().simple().to_string(),
            state: LeaseState::Allocating.as_str().to_string(),
            owner_host: holder.host,
            owner_pid: holder.pid,
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

    /// Whether the lease's owner is gone, so the lease may be taken over.
    ///
    /// Released (no pid) is gone. A process on this host is gone when its
    /// pid no longer exists; this process's own invocation is gone when it
    /// has ended. A process on another host cannot be observed from here and
    /// is held: [`ProviderLease::holder`] names it in the refusal. A lease
    /// from an older Stado with no pid is held to the expiry it stated.
    pub fn owner_expired(&self) -> Result<bool, LeaseError> {
        if self.owner_pid == 0 {
            if self.owner_expires_at.is_empty() {
                return Ok(true);
            }
            return Ok(Utc::now() >= parse_timestamp(&self.owner_expires_at)?);
        }
        let holder = ProcessOwner {
            host: self.owner_host.clone(),
            pid: self.owner_pid,
        };
        Ok(match holder.state() {
            OwnerState::Elsewhere | OwnerState::Running => false,
            OwnerState::Gone => true,
            OwnerState::This => !OwnerInvocation::running(&self.owner_id),
        })
    }

    /// Who holds the lease, said for a refusal.
    pub fn holder(&self) -> String {
        if self.owner_pid == 0 {
            return format!(
                "owner {} (stated expiry {:?})",
                self.owner_id, self.owner_expires_at
            );
        }
        format!(
            "owner {} (pid {} on {})",
            self.owner_id, self.owner_pid, self.owner_host
        )
    }

    /// Python `assert_fence`: owner_id + fence_token must match and the
    /// owner must still be live.
    pub fn assert_fence(&self, owner_id: &str, fence_token: &str) -> Result<(), LeaseError> {
        if self.owner_id != owner_id || self.fence_token != fence_token || self.owner_expired()? {
            return Err(LeaseError::conflict(
                "provider lease fence is no longer valid",
            ));
        }
        Ok(())
    }

    /// Python `renew_owner`: the owner says it is still working on the lease.
    /// Ownership itself lasts while the owner process does.
    pub fn renew_owner(&mut self, owner_id: &str, fence_token: &str) -> Result<(), LeaseError> {
        self.assert_fence(owner_id, fence_token)?;
        self.updated_at = now_iso();
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

    /// Python `takeover`: re-owner a lease whose owner is gone, rotating the
    /// fence token.
    pub fn takeover(&mut self, owner_id: &str) -> Result<(), LeaseError> {
        if !self.owner_expired()? {
            return Err(LeaseError::conflict(&format!(
                "provider lease is held by {}",
                self.holder()
            )));
        }
        let holder = ProcessOwner::current();
        self.owner_id = owner_id.to_string();
        self.fence_token = Uuid::new_v4().simple().to_string();
        self.owner_host = holder.host;
        self.owner_pid = holder.pid;
        self.owner_expires_at = String::new();
        self.updated_at = now_iso();
        Ok(())
    }

    /// Python `relinquish`: fence-gated immediate release.
    pub fn relinquish(&mut self, owner_id: &str, fence_token: &str) -> Result<(), LeaseError> {
        self.assert_fence(owner_id, fence_token)?;
        self.owner_host = String::new();
        self.owner_pid = 0;
        self.owner_expires_at = String::new();
        self.updated_at = now_iso();
        Ok(())
    }
}
