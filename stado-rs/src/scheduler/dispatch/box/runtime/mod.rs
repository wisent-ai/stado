//! Restartable structured execution inside an allocated Box.
//!
//! Port of `stado/scheduler/dispatch/box/runtime.py`.

mod control;
mod reconcile;
mod start;
mod terminal;

use chrono::{DateTime, Utc};

use crate::providers::r#box::BoxProvider;
use crate::queue::leases::{ProviderLease, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::BoxDispatchError;

const CONTROL_TIMEOUT_SECONDS: i64 = 60;
const PROMPT_RECOVERY_SECONDS: i64 = 120;
/// Python `_keepalive`'s `int("300")` owner-TTL renewal.
const KEEPALIVE_OWNER_TTL_SECONDS: i64 = 300;

/// Python `datetime.now(timezone.utc).isoformat()`.
pub(crate) fn now_iso() -> String {
    crate::models::isoformat_utc(Utc::now())
}

/// Python `datetime.fromisoformat(value.replace("Z", "+00:00"))`, lenient
/// (None when the stored timestamp is unparseable).
pub(crate) fn parse_iso(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value.replace('Z', "+00:00"))
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Lease owner-TTL renewal handed to the output helpers so long
/// paginations/uploads renew between network calls, exactly like Python's
/// `keepalive` callable parameter.
pub(crate) struct Keepalive<'r, 'l> {
    runtime: &'r BoxRuntime<'r>,
    lease: &'l mut ProviderLease,
}

impl Keepalive<'_, '_> {
    pub(crate) async fn ping(&mut self) -> Result<(), BoxDispatchError> {
        self.runtime.keepalive(self.lease).await
    }
}

/// Python `BoxRuntime`.
pub(crate) struct BoxRuntime<'a> {
    store: &'a JobStorage,
    provider: &'a BoxProvider,
    leases: &'a ProviderLeaseStore,
}

impl<'a> BoxRuntime<'a> {
    pub(crate) fn new(
        store: &'a JobStorage,
        provider: &'a BoxProvider,
        leases: &'a ProviderLeaseStore,
    ) -> Self {
        BoxRuntime {
            store,
            provider,
            leases,
        }
    }

    async fn save(&self, lease: &mut ProviderLease) -> Result<(), BoxDispatchError> {
        let version = lease.version.clone();
        *lease = self.leases.save(lease.clone(), &version).await?;
        Ok(())
    }

    /// Python `_keepalive`.
    async fn keepalive(&self, lease: &mut ProviderLease) -> Result<(), BoxDispatchError> {
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.renew_owner(&owner, &token, KEEPALIVE_OWNER_TTL_SECONDS)?;
        self.save(lease).await
    }

    /// A Keepalive handle borrowing this runtime and the lease.
    fn keepalive_handle<'r, 'l>(&'r self, lease: &'l mut ProviderLease) -> Keepalive<'r, 'l> {
        Keepalive {
            runtime: self,
            lease,
        }
    }
}
