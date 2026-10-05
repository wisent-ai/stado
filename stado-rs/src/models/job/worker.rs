//! Worker-origin observations, distinct from requested placement and queue refs.

use crate::capabilities::ProviderId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerAllocation {
    pub host: String,
    pub kind: String,
    pub observed_at: String,
    pub resource: Option<WorkerResource>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum WorkerResource {
    /// The worker's explicit local role; its rate remains a declared policy.
    Local,
    Aws {
        account_id: String,
        region: String,
        instance_id: String,
    },
    Gcp {
        project_id: String,
        zone: String,
        name: String,
        instance_id: u64,
    },
    Azure {
        subscription_id: String,
        location: String,
        name: String,
        resource_id: String,
        vm_id: String,
    },
}

impl WorkerResource {
    pub fn provider(&self) -> ProviderId {
        match self {
            Self::Local => ProviderId::Local,
            Self::Aws { .. } => ProviderId::Aws,
            Self::Gcp { .. } => ProviderId::Gcp,
            Self::Azure { .. } => ProviderId::Azure,
        }
    }
}
