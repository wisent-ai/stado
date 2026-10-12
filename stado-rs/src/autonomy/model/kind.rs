//! What kind of resource an inventory record is. Every inventory source maps
//! its provider's own types onto these once; the serde names are the words
//! stored snapshots, policy rules and canonical resource ids carry.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Instance,
    PersistentDisk,
    ManagedDisk,
    Volume,
    Snapshot,
    PublicIp,
    MachineImage,
    ObjectBucket,
    ObjectStorage,
    ContainerRegistry,
    Database,
    ScaleSet,
    ManagedInstanceGroup,
    LoadBalancer,
    NetworkInterface,
    Reservation,
    ServiceAccount,
    Build,
    Schedule,
    Function,
    Service,
    LocalHost,
    /// The advisor's synthetic record for the fleet's commitment portfolio.
    CommitmentPortfolio,
    /// A provider type no mapping names.
    #[serde(rename = "resource")]
    Other,
}

impl ResourceKind {
    /// Block storage a VM attaches.
    pub fn disk(self) -> bool {
        matches!(
            self,
            Self::PersistentDisk | Self::ManagedDisk | Self::Volume
        )
    }

    /// Stored artifacts whose retention is a lifecycle policy: images,
    /// buckets, storage accounts and registries.
    pub fn artifact(self) -> bool {
        matches!(
            self,
            Self::MachineImage | Self::ObjectBucket | Self::ObjectStorage | Self::ContainerRegistry
        )
    }

    /// Holds data that is lost when the resource is deleted.
    pub fn stateful(self) -> bool {
        self == Self::Database || self.disk()
    }
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(Value::String(word)) => formatter.write_str(&word),
            _ => Err(std::fmt::Error),
        }
    }
}
