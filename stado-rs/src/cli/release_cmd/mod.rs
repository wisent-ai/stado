//! `stado release ...` — signed immutable product release publication,
//! promotion, host reconciliation, status, and rollback.

mod commands;
mod fetch;
mod local;
mod publication;
mod rollout;
mod version_gate;

pub use commands::dispatch::dispatch;
pub use commands::{
    ReleaseActivateStagedArgs, ReleaseCommands, ReleaseDeclareVersionArgs, ReleaseHostStateArgs,
    ReleasePromoteVersionArgs, ReleaseProvenanceArgs, ReleaseProxyArgs, ReleaseVerifyPlatformArgs,
};
pub use fetch::ReleaseFetchArgs;
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

/// The refusal for a product name the release control plane does not hold.
///
/// It is the caller's request that names nothing, so it is stated as a
/// refusal rather than left to be read as an unattributable failure, and the
/// help lists the products configured in release control, so a typo is
/// visible at once. A name missing from that list may still have published
/// releases (Stado's own releases are delivered through `stado bootstrap`,
/// not through release control), so the help says only what is configured.
pub(crate) fn unknown_release_product(
    control: &crate::release_control::ReleaseControl,
    product: &str,
) -> crate::cli::CmdError {
    let configured: Vec<&str> = control.products.keys().map(String::as_str).collect();
    crate::cli::CmdError::click(format!("unknown release product {product:?}"))
        .stating(crate::primitives::failure::FailureCode::Refused)
        .helping(format!(
            "products configured in release control: {}",
            configured.join(", ")
        ))
}
