//! `stado release ...` — signed immutable product release publication,
//! promotion, host reconciliation, status, and rollback.

mod commands;
mod local;
mod publication;
mod rollout;

pub use commands::dispatch::dispatch;
pub use commands::{
    ReleaseActivateStagedArgs, ReleaseCommands, ReleaseDeclareVersionArgs, ReleaseHostStateArgs,
    ReleasePromoteVersionArgs, ReleaseProvenanceArgs, ReleaseProxyArgs, ReleaseVerifyPlatformArgs,
};
pub use local::{ReleaseConvergeLocalReadersArgs, ReleaseInstallLocalArgs};
pub use publication::{ReleaseClaimCoordinateArgs, ReleaseKeygenArgs, ReleasePrepareArgs};
pub use rollout::{
    ReleaseActiveBinaryArgs, ReleaseAgentArgs, ReleasePolicyApplyArgs, ReleasePromoteArgs,
    ReleaseRollbackArgs, ReleaseStatusArgs,
};

pub(crate) use publication::claims::claim_release_coordinate;
pub(crate) use publication::publish::{publish_pipeline_release, PipelinePublishRequest};
pub(crate) use publication::verification::{verified_artifact, verified_artifact_for_submit};
pub(crate) use rollout::promote::promote_for_submit;
