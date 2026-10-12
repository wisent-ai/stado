//! A Nebius instance's `status.state`, as its compute API lists it.

use crate::providers::gpu_cloud::api::MachineState;

/// A state outside the list, or none yet, is an instance still coming up.
#[derive(serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum Status {
    Running,
    Stopping,
    Stopped,
    Deleting,
    Error,
    #[serde(other)]
    Other,
}

impl From<Status> for MachineState {
    fn from(status: Status) -> Self {
        match status {
            Status::Running => Self::Running,
            Status::Stopping => Self::Stopping,
            Status::Stopped => Self::Stopped,
            Status::Deleting => Self::Terminating,
            Status::Error => Self::Failed,
            Status::Other => Self::Provisioning,
        }
    }
}
