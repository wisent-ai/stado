//! Internal routing facets, and how a facet's variants are selected.

use serde::Serialize;

use crate::capabilities::catalog::CapabilityKind;

/// Internal routing facets retained separately from user-facing capabilities.
/// These values organize adapters and configuration; they are not the product
/// capability list returned by [`product_capabilities`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeFacet {
    Compute,
    Storage,
    Execution,
    Scheduling,
    Inventory,
    Quota,
    Billing,
    Artifacts,
    Authentication,
    Secrets,
    Alerts,
    Deployment,
    HostTarget,
    Dependency,
}

impl RuntimeFacet {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compute => "compute",
            Self::Storage => "storage",
            Self::Execution => "execution",
            Self::Scheduling => "scheduling",
            Self::Inventory => "inventory",
            Self::Quota => "quota",
            Self::Billing => "billing",
            Self::Artifacts => "artifacts",
            Self::Authentication => "authentication",
            Self::Secrets => "secrets",
            Self::Alerts => "alerts",
            Self::Deployment => "deployment",
            Self::HostTarget => "host-target",
            Self::Dependency => "dependency",
        }
    }

    pub const fn product_capability(self) -> CapabilityKind {
        match self {
            Self::Compute => CapabilityKind::Compute,
            Self::Storage => CapabilityKind::ObjectStorage,
            Self::Execution => CapabilityKind::WorkloadExecution,
            Self::Scheduling => CapabilityKind::Scheduling,
            Self::Inventory => CapabilityKind::Inventory,
            Self::Quota => CapabilityKind::QuotaCapacity,
            Self::Billing => CapabilityKind::BillingCost,
            Self::Artifacts => CapabilityKind::ArtifactDistribution,
            Self::Authentication => CapabilityKind::IdentityAccess,
            Self::Secrets => CapabilityKind::Secrets,
            Self::Alerts => CapabilityKind::Messaging,
            Self::Deployment => CapabilityKind::ApplicationHosting,
            Self::HostTarget => CapabilityKind::Inventory,
            Self::Dependency => CapabilityKind::BackupRecovery,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionMode {
    Single,
    OrderedMany,
    ConcurrentMany,
    Automatic,
    Internal,
}

impl SelectionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::OrderedMany => "ordered-many",
            Self::ConcurrentMany => "concurrent-many",
            Self::Automatic => "automatic",
            Self::Internal => "internal",
        }
    }
}
