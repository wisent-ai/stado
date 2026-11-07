//! Execution, inventory, quota, billing and dependency adapters, and the
//! runtime adapter that unions every family.

use crate::capabilities::catalog::ProviderId;

use super::compute::{ComputeAdapter, GpuCloudVendor};
use super::storage::StorageAdapter;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionAdapter {
    Local,
    Gcp,
    Azure,
    Aws,
    Boat,
    Vast,
    /// An agent on a machine rented from a GPU cloud vendor.
    GpuCloud(GpuCloudVendor),
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
            Self::Boat => ProviderId::Boat,
            Self::Vast => ProviderId::Vast,
            Self::GpuCloud(vendor) => vendor.provider(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryAdapter {
    Gcp,
    Azure,
    Aws,
    /// The agent machines a GPU cloud vendor's credential can see.
    GpuCloud(GpuCloudVendor),
}

impl InventoryAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
            Self::GpuCloud(vendor) => vendor.provider(),
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
    /// The status of the hosted projects declared databases live in.
    Supabase,
}

impl BillingAdapter {
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Supabase => ProviderId::Supabase,
        }
    }

    /// Whether this source's section reports money (spend, credit, balance).
    /// Supabase's reports whether the projects behind declared databases run,
    /// so no spend or balance reader counts it and its health alert speaks
    /// of the databases, not of a disarmed credit threshold.
    pub const fn reports_money(self) -> bool {
        match self {
            Self::Gcp | Self::Azure => true,
            Self::Supabase => false,
        }
    }

    /// The billing adapter the catalog declares for `provider`, read once
    /// here so no consumer re-spells the catalog lookup.
    pub fn of(provider: &str) -> Option<Self> {
        match crate::capabilities::variant(crate::capabilities::RuntimeFacet::Billing, provider)
            .map(|variant| variant.adapter)
        {
            Some(RuntimeAdapter::Billing(adapter)) => Some(adapter),
            _ => None,
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
