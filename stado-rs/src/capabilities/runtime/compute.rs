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
}

impl ComputeAdapter {
    pub const fn tracks_cloud_cost(self) -> bool {
        matches!(self, Self::Gcp | Self::Azure | Self::Aws)
    }

    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcp => ProviderId::Gcp,
            Self::Azure => ProviderId::Azure,
            Self::Aws => ProviderId::Aws,
            Self::Box => ProviderId::Box,
            Self::ExistingHost => ProviderId::Local,
            Self::VastHost => ProviderId::Vast,
        }
    }
}
