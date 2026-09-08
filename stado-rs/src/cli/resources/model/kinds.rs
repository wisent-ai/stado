//! The closed vocabularies an operation is written in: the intent that
//! authorises it, the provider it addresses, the disposition of a finding, how
//! an action is authorised, how far it can be undone, and the action kinds
//! themselves — together with the intent each kind is admissible under.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u8 = true as u8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    RationalizationCleanup,
    AutonomousReconcile,
    Shutdown,
}

pub use crate::capabilities::ProviderId as ProviderKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingDisposition {
    Automatic,
    ReviewRequired,
    Blocked,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authorization {
    Automatic,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    Reversible,
    SnapshotRestore,
    Irreversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    DeleteInstance,
    SnapshotDisk,
    DeleteDisk,
    ReleaseAddress,
    DeleteManagedInstanceGroup,
    ReleaseReservation,
    DisableStorageBackup,
    PauseScheduler,
    ResizeManagedInstanceGroup,
    StopInstance,
    SuspendCloudSql,
    DeleteSnapshot,
    RestoreDisk,
    EnableStorageBackup,
    ResumeScheduler,
    StartInstance,
    RestoreCloudSql,
}

impl ActionKind {
    pub fn allowed_for(self, intent: Intent) -> bool {
        match intent {
            Intent::RationalizationCleanup => matches!(
                self,
                Self::DeleteInstance
                    | Self::SnapshotDisk
                    | Self::DeleteDisk
                    | Self::ReleaseAddress
                    | Self::DeleteManagedInstanceGroup
                    | Self::ReleaseReservation
                    | Self::DisableStorageBackup
            ),
            Intent::AutonomousReconcile => matches!(
                self,
                Self::DeleteInstance | Self::StopInstance | Self::StartInstance
            ),
            Intent::Shutdown => matches!(
                self,
                Self::PauseScheduler
                    | Self::ResizeManagedInstanceGroup
                    | Self::StopInstance
                    | Self::SuspendCloudSql
            ),
        }
    }
}
