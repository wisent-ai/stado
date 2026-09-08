//! Execution, inventory, quota, billing and dependency adapters, and the
//! runtime adapter that unions every family.

use crate::capabilities::catalog::ProviderId;

use super::compute::ComputeAdapter;
use super::storage::StorageAdapter;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionAdapter {
    Local,
    Gcp,
    Azure,
    Aws,
    Box,
    Vast,
}

impl ExecutionAdapter {
    pub const fn allows_job_system_packages(self) -> bool {
        !matches!(self, Self::Local)
    }

    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Local => ProviderId::Local,
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
            Self::Box => ProviderId::Box,
            Self::Vast => ProviderId::Vast,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryAdapter {
    Gcp,
    Azure,
    Aws,
}

impl InventoryAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaAdapter {
    Gcp,
    Azure,
    StorageOverlay,
}

impl QuotaAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::StorageOverlay => ProviderId::Stado,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BillingAdapter {
    Gcp,
    Azure,
}

impl BillingAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencyAdapter {
    Gcp,
    Azure,
    Aws,
    Local,
}

impl DependencyAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
            Self::Local => ProviderId::Local,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeAdapter {
    None,
    Compute(ComputeAdapter),
    Storage(StorageAdapter),
    Execution(ExecutionAdapter),
    Inventory(InventoryAdapter),
    Quota(QuotaAdapter),
    Billing(BillingAdapter),
    Dependency(DependencyAdapter),
}

impl RuntimeAdapter {
    pub const fn provider(self) -> Option<ProviderId> {
        match self {
            Self::None => None,
            Self::Compute(adapter) => Some(adapter.provider()),
            Self::Storage(adapter) => Some(adapter.provider()),
            Self::Execution(adapter) => Some(adapter.provider()),
            Self::Inventory(adapter) => Some(adapter.provider()),
            Self::Quota(adapter) => Some(adapter.provider()),
            Self::Billing(adapter) => Some(adapter.provider()),
            Self::Dependency(adapter) => Some(adapter.provider()),
        }
    }
}
