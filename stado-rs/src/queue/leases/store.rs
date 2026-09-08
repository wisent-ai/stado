//! Conditional persistence of a lease document: the path it lives at, the
//! byte shape it is stored in, and the create/CAS/takeover writes that are
//! the only way a lease ever changes on the backend.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

use super::{LeaseError, ProviderLease};

/// Python `_MAX_LEASE_BYTES`.
const MAX_LEASE_BYTES: usize = 65536;

/// Python `_SAFE_JOB_ID`.
static SAFE_JOB_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+$").expect("static regex compiles"));

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
