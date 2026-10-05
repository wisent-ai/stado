//! Restartable structured execution inside an allocated Box.
//!
//! Port of `stado/scheduler/dispatch/box/runtime.py`.

mod control;
mod reconcile;
mod start;
mod terminal;

use chrono::Utc;

use crate::providers::r#box::BoxProvider;
use crate::queue::leases::{ProviderLease, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::BoxDispatchError;

/// Python `datetime.now(timezone.utc).isoformat()`.
pub(crate) fn now_iso() -> String {
    crate::models::isoformat_utc(Utc::now())
}

/// Lease fence check handed to the output helpers so long
/// paginations/uploads confirm between network calls that this invocation
/// still owns the lease, exactly like Python's `keepalive` callable.
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

    /// Python `_keepalive`: confirm and record that the owner still works.
    async fn keepalive(&self, lease: &mut ProviderLease) -> Result<(), BoxDispatchError> {
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.renew_owner(&owner, &token)?;
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
