//! Fenced provider-resource leases stored with backend compare-and-swap.
//!
//! Port of `stado/queue/leases/__init__.py`. A lease is a single JSON blob at
//! `provider-leases/{job_id}.json` guarded by an (owner_id, fence_token) pair
//! plus owner/resource TTLs; all mutations go through the backend's
//! conditional-write primitives so a stale owner loses the race.
//!
//! Known Python bug (ported as INTENDED, not as written):
//! `leases/__init__.py:143` gates every store operation on
//! `_require_conditional_backend()`, which checks
//! `getattr(storage, "_azure_backend", None)` — an attribute that never
//! exists on Python `JobStorage` (the backend handle is `_blob_backend`), so
//! the check always raises unless the GCS SDK path is present. The intended
//! behavior is "the backend supports conditional writes", which is true for
//! every Rust [`BlobBackend`] (CAS is part of the trait contract), so the
//! gate is a no-op here.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::LazyLock;

use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::storage::JobStorage;
use super::StorageError;

/// Python `_MAX_LEASE_BYTES`.
const MAX_LEASE_BYTES: usize = 65536;

/// Python `_SAFE_JOB_ID`.
static SAFE_JOB_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+$").expect("static regex compiles"));

/// Lease-layer error. Python raises `LeaseConflict` (a `RuntimeError`
/// subclass) for lost races and invalid fences, `ValueError` for illegal
/// transitions / unsafe job ids / bad timestamps, and `RuntimeError` for
/// size and shape violations of the stored blob.
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// Python `LeaseConflict`.
    #[error("{0}")]
    Conflict(String),
    /// Python `ValueError`.
    #[error("{0}")]
    Value(String),
    /// Python `RuntimeError` for a corrupt/oversized stored lease.
    #[error("{0}")]
    Corrupt(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl LeaseError {
    fn conflict(message: &str) -> Self {
        LeaseError::Conflict(message.to_string())
    }

    /// Whether this is a `LeaseConflict` (Python `except LeaseConflict`).
    pub fn is_conflict(&self) -> bool {
        matches!(self, LeaseError::Conflict(_))
    }
}

/// Python `LeaseState` (str Enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    Allocating,
    Provisioning,
    Ready,
    Starting,
    Running,
    Collecting,
    Releasing,
    Released,
    Failed,
}

impl LeaseState {
    /// The serialized value (Python `.value`).
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseState::Allocating => "allocating",
            LeaseState::Provisioning => "provisioning",
            LeaseState::Ready => "ready",
            LeaseState::Starting => "starting",
            LeaseState::Running => "running",
            LeaseState::Collecting => "collecting",
            LeaseState::Releasing => "releasing",
            LeaseState::Released => "released",
            LeaseState::Failed => "failed",
        }
    }

    /// Python `_ALLOWED_TRANSITIONS`.
    fn allowed_transitions(self) -> &'static [LeaseState] {
        match self {
            LeaseState::Allocating => &[LeaseState::Provisioning, LeaseState::Failed],
            LeaseState::Provisioning => &[LeaseState::Ready, LeaseState::Failed],
            LeaseState::Ready => &[LeaseState::Starting, LeaseState::Failed],
            LeaseState::Starting => &[LeaseState::Running, LeaseState::Failed],
            LeaseState::Running => &[LeaseState::Collecting, LeaseState::Failed],
            LeaseState::Collecting => &[LeaseState::Releasing, LeaseState::Failed],
            LeaseState::Releasing => &[LeaseState::Released, LeaseState::Failed],
            LeaseState::Failed => &[LeaseState::Releasing, LeaseState::Released],
            LeaseState::Released => &[],
        }
    }
}

impl FromStr for LeaseState {
    type Err = LeaseError;

    /// Python `LeaseState(value)` (raises `ValueError` on unknown states).
    fn from_str(value: &str) -> Result<Self, LeaseError> {
        let state = match value {
            "allocating" => LeaseState::Allocating,
            "provisioning" => LeaseState::Provisioning,
            "ready" => LeaseState::Ready,
            "starting" => LeaseState::Starting,
            "running" => LeaseState::Running,
            "collecting" => LeaseState::Collecting,
            "releasing" => LeaseState::Releasing,
            "released" => LeaseState::Released,
            "failed" => LeaseState::Failed,
            other => {
                return Err(LeaseError::Value(format!(
                    "{other:?} is not a valid LeaseState"
                )));
            }
        };
        Ok(state)
    }
}

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
    fn to_value(&self) -> serde_json::Value {
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

/// Python `ProviderLeaseStore`: conditional persistence over the configured
/// JobStorage backend.
pub struct ProviderLeaseStore {
    storage: JobStorage,
}

impl ProviderLeaseStore {
    pub fn new(job_storage: JobStorage) -> Self {
        ProviderLeaseStore {
            storage: job_storage,
        }
    }

    /// The wrapped facade.
    pub fn storage(&self) -> &JobStorage {
        &self.storage
    }

    // Python `_require_conditional_backend` is NOT ported as a runtime gate:
    // it reads `storage._azure_backend`, which never exists (the Python bug
    // noted in the module docs). The intended precondition — the backend
    // supports create-if-absent and compare-and-swap — holds for every Rust
    // `BlobBackend` by construction.

    /// Python `_path`.
    fn path(job_id: &str) -> Result<String, LeaseError> {
        if !SAFE_JOB_ID.is_match(job_id) {
            return Err(LeaseError::Value(
                "job id is unsafe for provider lease storage".to_string(),
            ));
        }
        Ok(format!("provider-leases/{job_id}.json"))
    }

    /// Python `_encode`: `json.dumps(to_dict(), separators=(",", ":"),
    /// sort_keys=True)`.
    fn encode(lease: &ProviderLease) -> String {
        let serde_json::Value::Object(map) = lease.to_value() else {
            unreachable!("ProviderLease serializes to an object");
        };
        let sorted: BTreeMap<String, serde_json::Value> = map.into_iter().collect();
        crate::models::ensure_ascii(
            &serde_json::to_string(&sorted).expect("lease serialization is infallible"),
        )
    }

    /// Python `_decode`.
    fn decode(raw: &str, version: &str) -> Result<ProviderLease, LeaseError> {
        if raw.len() > MAX_LEASE_BYTES {
            return Err(LeaseError::Corrupt(
                "provider lease exceeded size bound".to_string(),
            ));
        }
        let value: serde_json::Value = serde_json::from_str(raw).map_err(StorageError::Json)?;
        if !value.is_object() {
            return Err(LeaseError::Corrupt(
                "provider lease is not an object".to_string(),
            ));
        }
        let mut lease: ProviderLease = serde_json::from_value(value).map_err(StorageError::Json)?;
        lease.version = version.to_string();
        Ok(lease)
    }

    /// Python `load`.
    pub async fn load(&self, job_id: &str) -> Result<Option<ProviderLease>, LeaseError> {
        let Some(value) = self
            .storage
            .read_text_versioned(&Self::path(job_id)?)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(Self::decode(&value.content, &value.version)?))
    }

    /// Python `create`: atomic create-if-absent, then re-read to confirm the
    /// blob that landed is the one we wrote.
    pub async fn create(&self, mut lease: ProviderLease) -> Result<ProviderLease, LeaseError> {
        let path = Self::path(&lease.job_id)?;
        if !self
            .storage
            .create_text_if_absent(&path, &Self::encode(&lease))
            .await?
        {
            return Err(LeaseError::conflict("provider lease already exists"));
        }
        let Some(created) = self.load(&lease.job_id).await? else {
            return Err(LeaseError::conflict(
                "provider lease disappeared after creation",
            ));
        };
        if created.to_value() != lease.to_value() {
            return Err(LeaseError::conflict(
                "provider lease changed before creation was confirmed",
            ));
        }
        lease.version = created.version;
        Ok(lease)
    }

    /// Python `save`: compare-and-swap against the version this owner read.
    pub async fn save(
        &self,
        mut lease: ProviderLease,
        expected_version: &str,
    ) -> Result<ProviderLease, LeaseError> {
        if expected_version.is_empty() || lease.version != expected_version {
            return Err(LeaseError::conflict(
                "provider lease version was not read by this owner",
            ));
        }
        let new_version = match self
            .storage
            .compare_and_swap_text(
                &Self::path(&lease.job_id)?,
                expected_version,
                &Self::encode(&lease),
            )
            .await
        {
            Ok(version) => version,
            // Python `except StorageConflict: raise LeaseConflict(...)`.
            Err(StorageError::StorageConflict(_)) => {
                return Err(LeaseError::conflict("provider lease changed concurrently"));
            }
            Err(err) => return Err(err.into()),
        };
        if new_version.is_empty() {
            return Err(LeaseError::Corrupt(
                "conditional lease write did not return a version".to_string(),
            ));
        }
        lease.version = new_version;
        Ok(lease)
    }

    /// Python `acquire`: create the lease, or — when it already exists and
    /// the recorded owner TTL has lapsed — take it over via CAS.
    pub async fn acquire(
        &self,
        job_id: &str,
        provider: &str,
        owner_id: &str,
        owner_ttl_seconds: i64,
        resource_ttl_seconds: i64,
    ) -> Result<ProviderLease, LeaseError> {
        let lease = ProviderLease::new(
            job_id,
            provider,
            owner_id,
            owner_ttl_seconds,
            resource_ttl_seconds,
        );
        match self.create(lease).await {
            Ok(created) => Ok(created),
            Err(err) if err.is_conflict() => {
                let Some(mut current) = self.load(job_id).await? else {
                    return Err(LeaseError::conflict(
                        "provider lease disappeared during acquisition",
                    ));
                };
                if current.job_id != job_id || current.provider != provider {
                    return Err(LeaseError::conflict("provider lease identity mismatch"));
                }
                if !current.owner_expired()? {
                    return Err(LeaseError::conflict("provider lease owner is still live"));
                }
                let version = current.version.clone();
                current.takeover(owner_id, owner_ttl_seconds)?;
                self.save(current, &version).await
            }
            Err(err) => Err(err),
        }
    }
}
