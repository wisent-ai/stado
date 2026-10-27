//! `stado release ...` — signed immutable product release publication,
//! promotion, host reconciliation, status, and rollback.

mod commands;
pub(crate) mod destinations;
mod failures;
mod fetch;
mod local;
mod publication;
mod rollout;
mod version_gate;

pub(crate) use rollout::policy::drop_policy_target;
pub(crate) use version_gate::{least_next, reached_modules, semver_order, undeclared_files};

pub use commands::dispatch::dispatch;
pub use commands::{
    ReleaseActivateStagedArgs, ReleaseCommands, ReleaseDeclareVersionArgs, ReleaseHostVersionArgs,
    ReleasePromoteVersionArgs, ReleaseProvenanceArgs, ReleaseProxyArgs, ReleaseUnsetVersionArgs,
    ReleaseVersionCommands,
};
pub use fetch::ReleaseFetchArgs;
pub(crate) use local::failed::{failed_here, roll_back_failed_release};
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
/// It is the caller's request that names nothing release control can answer
/// for, so it is stated as a refusal rather than left to be read as an
/// unattributable failure, and the help lists the products configured in
/// release control, so a typo is visible at once. A product missing from that
/// list may still have published releases: release control holds only the
/// products rolled out by policy, and Stado's own releases come through
/// `stado bootstrap`. Worded as "unknown release product", this refusal was
/// read again and again as "no release of it exists" for products whose
/// archives stood published, so it says what release control lacks and names
/// the read that lists the product's published releases.
pub(crate) fn unknown_release_product(
    control: &crate::release_control::ReleaseControl,
    product: &str,
) -> crate::cli::CmdError {
    let configured: Vec<&str> = control.products.keys().map(String::as_str).collect();
    crate::cli::CmdError::click(format!(
        "release control holds no rollout policy for {product:?}; this does not say whether a \
         release of it was published"
    ))
    .stating(crate::primitives::failure::FailureCode::Refused)
    .helping(format!(
        "products configured in release control: {}; the published releases of {product:?}, if \
         any, are listed by `stado storage objects releases {product}/`",
        configured.join(", ")
    ))
}
