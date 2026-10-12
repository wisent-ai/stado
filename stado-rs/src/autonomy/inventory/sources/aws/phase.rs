//! What each EC2 state the SDK reports means for a resource's lifecycle
//! phase. The SDK enums are non-exhaustive, so a state AWS adds later is
//! unknown until it is placed here.

use aws_sdk_ec2::types::{
    ImageState, InstanceStateName, ReservedInstanceState, SnapshotState, VolumeState,
};

use crate::providers::LifecyclePhase;

pub(super) fn instance(state: Option<&InstanceStateName>) -> LifecyclePhase {
    match state {
        Some(InstanceStateName::Pending | InstanceStateName::Running) => LifecyclePhase::Running,
        Some(InstanceStateName::Stopping | InstanceStateName::Stopped) => LifecyclePhase::Stopped,
        Some(InstanceStateName::ShuttingDown | InstanceStateName::Terminated) => {
            LifecyclePhase::Ended
        }
        _ => LifecyclePhase::Unknown,
    }
}

pub(super) fn volume(state: Option<&VolumeState>) -> LifecyclePhase {
    match state {
        Some(VolumeState::Creating | VolumeState::Available | VolumeState::InUse) => {
            LifecyclePhase::Running
        }
        Some(VolumeState::Deleting | VolumeState::Deleted | VolumeState::Error) => {
            LifecyclePhase::Ended
        }
        _ => LifecyclePhase::Unknown,
    }
}

pub(super) fn snapshot(state: Option<&SnapshotState>) -> LifecyclePhase {
    match state {
        Some(
            SnapshotState::Pending
            | SnapshotState::Completed
            | SnapshotState::Recoverable
            | SnapshotState::Recovering,
        ) => LifecyclePhase::Running,
        Some(SnapshotState::Error) => LifecyclePhase::Ended,
        _ => LifecyclePhase::Unknown,
    }
}

pub(super) fn image(state: Option<&ImageState>) -> LifecyclePhase {
    match state {
        Some(
            ImageState::Available
            | ImageState::Pending
            | ImageState::Transient
            | ImageState::Disabled,
        ) => LifecyclePhase::Running,
        Some(
            ImageState::Failed | ImageState::Deregistered | ImageState::Error | ImageState::Invalid,
        ) => LifecyclePhase::Ended,
        _ => LifecyclePhase::Unknown,
    }
}

pub(super) fn reservation(state: Option<&ReservedInstanceState>) -> LifecyclePhase {
    match state {
        Some(
            ReservedInstanceState::Active
            | ReservedInstanceState::PaymentPending
            | ReservedInstanceState::Queued,
        ) => LifecyclePhase::Running,
        Some(
            ReservedInstanceState::Retired
            | ReservedInstanceState::PaymentFailed
            | ReservedInstanceState::QueuedDeleted,
        ) => LifecyclePhase::Ended,
        _ => LifecyclePhase::Unknown,
    }
}
