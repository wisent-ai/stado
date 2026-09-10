//! The `stado release` subcommand surface: the command enum, the argument
//! records whose only reader is the dispatch below, and the dispatch itself.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use super::local::{ReleaseConvergeLocalReadersArgs, ReleaseInstallLocalArgs};
use super::publication::{ReleaseClaimCoordinateArgs, ReleaseKeygenArgs, ReleasePrepareArgs};
use super::rollout::{
    ReleaseActiveBinaryArgs, ReleaseAgentArgs, ReleasePolicyApplyArgs, ReleasePromoteArgs,
    ReleaseRollbackArgs, ReleaseStatusArgs,
};

pub(super) mod dispatch;

// A clap subcommand enum is constructed once per process from parsed argv, so
// the largest variant costs one stack frame at startup and boxing it would only
// add an allocation and a deref to every match arm.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Generate an Ed25519 release authority key pair.
    Keygen(ReleaseKeygenArgs),
    /// Apply reviewed product policy without changing the active release.
    #[command(name = "policy-apply")]
    PolicyApply(ReleasePolicyApplyArgs),
    /// Snapshot, qualify, build, sign, publish, deliver, and promote a product.
    Submit(crate::cli::release_submit::ReleaseSubmitArgs),
    /// Resume a recorded release without replacing its source or running jobs.
    Resume(crate::cli::release_submit::ReleaseResumeArgs),
    /// Re-run one delivery from an exact completed release without promotion.
    Redeliver(crate::cli::release_submit::ReleaseRedeliverArgs),
    /// Manage the Stado-owned product and source policy catalog.
    Catalog(crate::cli::release_catalog::CatalogArgs),
    /// Internal provider-neutral release build worker.
    #[command(hide = true)]
    Worker(crate::cli::release_submit::ReleaseWorkerArgs),
    /// Internal provider-neutral post-publication delivery worker.
    #[command(name = "delivery-worker", hide = true)]
    DeliveryWorker(crate::cli::release_submit::DeliveryWorkerArgs),
    /// Build, sign, and publish one immutable candidate coordinate.
    Prepare(ReleasePrepareArgs),
    /// Promote exact qualified candidate bytes into registry desired state.
    Promote(ReleasePromoteArgs),
    /// Reconcile desired releases on this exact registry target.
    Agent(ReleaseAgentArgs),
    /// Internal stable-port proxy owned by the release agent.
    #[command(hide = true)]
    Proxy(ReleaseProxyArgs),
    /// Show desired and observed rollout state, and the register of recent
    /// publication attempts.
    ///
    /// The newest pipeline runs are listed with each platform's job, its
    /// state, and the recorded failure of anything that died - the builder
    /// that refused the work, the secret that could not be resolved, or the
    /// job's own last output - so a failed publication can be read back
    /// without opening the store. Stado Desktop shows the same register and
    /// the same failure text on its Releases screen.
    Status(ReleaseStatusArgs),
    /// Resolve the exact policy-derived executable of the active signed release.
    #[command(name = "active-binary")]
    ActiveBinary(ReleaseActiveBinaryArgs),
    /// Read a release candidate's own stdout/stderr off the target host.
    Logs(crate::cli::release_evidence::ReleaseLogsArgs),
    /// One verdict over desired state, the candidate, quarantine and the
    /// host's claiming gates.
    Doctor(crate::cli::release_evidence::ReleaseDoctorArgs),
    /// List and retire the digests a host refuses to roll out again.
    #[command(subcommand)]
    Quarantine(crate::cli::release_quarantine::QuarantineCommands),
    /// Atomically restore the previous desired release.
    Rollback(ReleaseRollbackArgs),
    /// Install a delivered release archive's binary on this very host.
    #[command(name = "install-local")]
    InstallLocal(ReleaseInstallLocalArgs),
    /// Reconcile live readers of an already-installed native binary.
    #[command(name = "converge-local-readers", hide = true)]
    ConvergeLocalReaders(ReleaseConvergeLocalReadersArgs),
    /// Bind one immutable coordinate to exactly one source revision before
    /// anything is published into it.
    #[command(name = "claim-coordinate")]
    ClaimCoordinate(ReleaseClaimCoordinateArgs),
    /// Set or remove a host's exact managed binary version declaration.
    #[command(name = "declare-version")]
    DeclareVersion(ReleaseDeclareVersionArgs),
    /// Verify and promote one published version into a host declaration.
    #[command(name = "promote-version")]
    PromoteVersion(ReleasePromoteVersionArgs),
    /// Activate one host's already-staged release with its own installer.
    #[command(name = "activate-staged")]
    ActivateStaged(ReleaseActivateStagedArgs),
    /// Run the native release journeys on one declared host platform.
    #[command(name = "verify-platform")]
    VerifyPlatform(ReleaseVerifyPlatformArgs),
    /// Read or converge the versions a host declares.
    #[command(name = "host-state")]
    HostState(ReleaseHostStateArgs),
    /// Attest the source and bytes of the release artifacts a host carries.
    Provenance(ReleaseProvenanceArgs),
}

/// Set or remove one managed-version declaration for a registry host.
#[derive(Args)]
pub struct ReleaseDeclareVersionArgs {
    /// Registry target whose declaration is changed.
    #[arg(long)]
    host: String,
    #[arg(long)]
    binary: String,
    /// Exact version to declare.
    #[arg(long, required_unless_present = "unset", conflicts_with = "unset")]
    version: Option<String>,
    /// Remove this binary's declaration instead of setting a version.
    #[arg(long, conflicts_with = "version")]
    unset: bool,
    #[arg(long)]
    json: bool,
}

/// Promote one published version into one host's declared desired state.
#[derive(Args)]
pub struct ReleasePromoteVersionArgs {
    /// Registry target whose declaration is promoted.
    #[arg(long)]
    host: String,
    #[arg(long)]
    binary: String,
    #[arg(long)]
    version: String,
    #[arg(long)]
    json: bool,
}

/// Activate a release already staged on a host.
#[derive(Args)]
pub struct ReleaseActivateStagedArgs {
    #[arg(long)]
    host: String,
    #[arg(long, default_value = "weles-worker")]
    product: String,
    #[arg(long, default_value = "$HOME/.config/weles/worker.env")]
    env_file: String,
    #[arg(long, default_value_t = 8788)]
    port: u16,
    #[arg(long)]
    json: bool,
}

/// Verify the declared platform by running the native release journeys.
#[derive(Args)]
pub struct ReleaseVerifyPlatformArgs {
    #[arg(long)]
    host: String,
    #[arg(long)]
    repo: String,
    #[arg(long = "ref")]
    revision: String,
    #[arg(long)]
    json: bool,
}

/// Report or converge one host against `targets[].managed_versions`.
#[derive(Args)]
pub struct ReleaseHostStateArgs {
    #[arg(long)]
    host: String,
    /// Limit the report to one declared binary.
    #[arg(long)]
    binary: Option<String>,
    /// Deliver host-behind versions; refuses to downgrade a host-ahead binary.
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    json: bool,
}

/// Report the release provenance of every managed artifact on one host.
#[derive(Args)]
pub struct ReleaseProvenanceArgs {
    #[arg(long)]
    host: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseProxyArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    bind: String,
}
