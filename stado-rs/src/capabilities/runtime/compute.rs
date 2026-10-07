//! The compute adapters behind machine provisioning.

use crate::capabilities::catalog::ProviderId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputeAdapter {
    Gcp,
    Azure,
    Aws,
    Box,
    ExistingHost,
    VastHost,
    /// A GPU or cloud compute vendor rented through the shared
    /// `providers::gpu_cloud` lifecycle.
    GpuCloud(GpuCloudVendor),
}

impl ComputeAdapter {
    pub const fn tracks_cloud_cost(self) -> bool {
        matches!(self, Self::Gcp | Self::Azure | Self::Aws)
    }

    /// Whether this adapter rents a machine per dispatch and boots a Stado
    /// agent on it: the coordinator checks, reaps and schedules it every
    /// tick, and the agent needs the release coordinates. A GPU cloud vendor
    /// whose API cannot hand a machine a startup script rents nothing here.
    pub const fn dispatches_agent_machines(self) -> bool {
        match self {
            Self::Gcp | Self::Azure | Self::Aws => true,
            Self::GpuCloud(vendor) => crate::providers::gpu_cloud::profile(vendor).boots_agent(),
            Self::Box | Self::ExistingHost | Self::VastHost => false,
        }
    }

    /// Whether Stado holds this provider's account credential, which may sit
    /// in the coordinator's grant only: a workload grant carrying it would
    /// hand every job the power to rent and destroy machines.
    pub const fn holds_cloud_credential(self) -> bool {
        matches!(
            self,
            Self::Gcp | Self::Azure | Self::Aws | Self::GpuCloud(_)
        )
    }
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
            Self::Box => ProviderId::Box,
            Self::ExistingHost => ProviderId::Local,
            Self::VastHost => ProviderId::Vast,
            Self::GpuCloud(vendor) => vendor.provider(),
        }
    }
}

/// The GPU and cloud compute vendors Stado rents agent machines from through
/// one shared lifecycle contract (`providers::gpu_cloud`). Each is a provider
/// in the catalog with its own id, credential role and configuration
/// section. GCP, Azure and AWS keep their dedicated adapters because their
/// quota, billing and inventory reach further than machine lifecycle.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GpuCloudVendor {
    Arkane,
    Crusoe,
    Cudo,
    Hyperstack,
    Lambda,
    Latitude,
    Nebius,
    Oblivus,
    Oracle,
    Runpod,
    Salad,
    Scaleway,
    VoltagePark,
    Vultr,
}

impl GpuCloudVendor {
    /// Every vendor, in catalog order.
    pub const ALL: &'static [GpuCloudVendor] = &[
        Self::Arkane,
        Self::Crusoe,
        Self::Cudo,
        Self::Hyperstack,
        Self::Lambda,
        Self::Latitude,
        Self::Nebius,
        Self::Oblivus,
        Self::Oracle,
        Self::Runpod,
        Self::Salad,
        Self::Scaleway,
        Self::VoltagePark,
        Self::Vultr,
    ];

    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Arkane => ProviderId::Arkane,
            Self::Crusoe => ProviderId::Crusoe,
            Self::Cudo => ProviderId::Cudo,
            Self::Hyperstack => ProviderId::Hyperstack,
            Self::Lambda => ProviderId::Lambda,
            Self::Latitude => ProviderId::Latitude,
            Self::Nebius => ProviderId::Nebius,
            Self::Oblivus => ProviderId::Oblivus,
            Self::Oracle => ProviderId::Oracle,
            Self::Runpod => ProviderId::Runpod,
            Self::Salad => ProviderId::Salad,
            Self::Scaleway => ProviderId::Scaleway,
            Self::VoltagePark => ProviderId::VoltagePark,
            Self::Vultr => ProviderId::Vultr,
        }
    }

    /// The vendor's own name, as an operator reads it in a refusal.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Arkane => "Arkane Cloud",
            Self::Crusoe => "Crusoe Cloud",
            Self::Cudo => "Cudo Compute",
            Self::Hyperstack => "Hyperstack",
            Self::Lambda => "Lambda Cloud",
            Self::Latitude => "Latitude.sh",
            Self::Nebius => "Nebius AI Cloud",
            Self::Oblivus => "Oblivus Cloud",
            Self::Oracle => "Oracle Cloud Infrastructure",
            Self::Runpod => "RunPod",
            Self::Salad => "SaladCloud",
            Self::Scaleway => "Scaleway",
            Self::VoltagePark => "Voltage Park",
            Self::Vultr => "Vultr",
        }
    }

    pub fn from_provider(provider: ProviderId) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|vendor| vendor.provider() == provider)
    }

    /// The Skarbiec role whose item holds this vendor's API credential.
    pub fn credential_role(self) -> String {
        cloud_credential_role(self.provider())
    }
}

/// The Skarbiec role a machine-renting provider's control-plane credential
/// plays: the item tagged `stado:role:cloud-<provider id>`. AWS, Azure and GCP
/// read `cloud-aws`, `cloud-azure` and `cloud-gcp` by the same rule.
pub fn cloud_credential_role(provider: ProviderId) -> String {
    format!("cloud-{}", provider.as_str())
}
