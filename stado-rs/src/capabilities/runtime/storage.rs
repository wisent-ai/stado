//! The storage adapters, and how far a coordinate written to one carries.

use crate::capabilities::catalog::ProviderId;
use crate::capabilities::registry::constructible_variant;

use super::facet::RuntimeFacet;
use super::services::RuntimeAdapter;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageAdapter {
    Gcs,
    AzureBlob,
    S3,
    StadoObject,
    Local,
}

/// Whether a store answers for the whole fleet or only for the machine it sits
/// on.
///
/// Written as data rather than inferred from a name so that adding a backend is
/// a decision the compiler asks for. It decides one thing: whether a coordinate
/// published here means the same object on every other host. A release is a
/// claim about the fleet, and a claim resting on a device store does not fail
/// -- it succeeds, and every other host reports the object absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageReach {
    /// Every host resolves the same coordinate to the same object.
    Fleet,
    /// The coordinate is meaningful only on the machine that wrote it.
    Device,
}

impl StorageAdapter {
    /// How far a coordinate written to this store carries.
    pub const fn reach(self) -> StorageReach {
        match self {
            Self::Gcs | Self::AzureBlob | Self::S3 | Self::StadoObject => StorageReach::Fleet,
            Self::Local => StorageReach::Device,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Gcs => "gcs",
            Self::AzureBlob => ProviderId::Azure.as_str(),
            Self::S3 => "s3",
            Self::StadoObject => ProviderId::Stado.as_str(),
            Self::Local => ProviderId::Local.as_str(),
        }
    }

    pub const fn required_backup(self) -> Option<Self> {
        match self {
            Self::AzureBlob => Some(Self::S3),
            _ => None,
        }
    }

    pub const fn provider(self) -> ProviderId {
        match self {
            Self::Gcs => ProviderId::Gcp,
            Self::AzureBlob => ProviderId::Azure,
            Self::S3 => ProviderId::Aws,
            Self::StadoObject => ProviderId::Stado,
            Self::Local => ProviderId::Local,
        }
    }
}

/// How far a coordinate written to the backend NAMED `backend` carries, or
/// `None` when this build does not know that backend.
///
/// The lookup exists once because two callers now depend on the same answer for
/// opposite reasons, and a second spelling of it would let them disagree.
/// `artifact publish` asks before writing a `stado://` coordinate. The queue
/// agent asks before broadcasting capacity: a successful write to device-local
/// storage does not make the publication addressable by the fleet.
pub fn storage_reach(backend: &str) -> Option<StorageReach> {
    match constructible_variant(RuntimeFacet::Storage, backend)?.adapter {
        RuntimeAdapter::Storage(adapter) => Some(adapter.reach()),
        _ => None,
    }
}
