//! The `stado release` subcommand surface: the command enum, the argument
//! records whose only reader is the dispatch below, and the dispatch itself.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use super::local::{
    ReleaseConvergeLocalReadersArgs, ReleaseInstallLocalArgs, ReleaseRestoreLocalArgs,
};
use super::publication::{ReleaseClaimCoordinateArgs, ReleaseKeygenArgs, ReleasePrepareArgs};
use super::rollout::{
    ReleaseActiveBinaryArgs, ReleaseAgentArgs, ReleasePolicyCommands, ReleasePromoteArgs,
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
    /// Apply, read and remove the reviewed rollout policy of each
    /// release-controlled product.
    #[command(subcommand)]
    Policy(ReleasePolicyCommands),
    /// Snapshot, qualify, build, sign, publish, deliver, and promote a product.
    Submit(crate::cli::release_submit::ReleaseSubmitArgs),
    /// Release every product in this workspace from the commit and version it
    /// already declares.
    ///
    /// One command for the whole workspace: each product checkout is read for
    /// the commit it stands on and the version that commit declares. A version
    /// a run published is skipped, a version a run of another commit holds is
    /// skipped with the new version it needs, and a commit a run is still
    /// releasing is skipped; a commit whose own run failed or was superseded
    /// is released again. The rest go through the same pipeline `submit`
    /// drives. `--plan` reads without submitting.
    Newest(crate::cli::release_newest::ReleaseNewestArgs),
    /// Hand pushed work to a later batch release, without starting a build.
    Changes(crate::cli::release_submit::changes::ChangesArgs),
    /// Resume a recorded release without replacing its source or running jobs.
    Resume(crate::cli::release_submit::ReleaseResumeArgs),
    /// Re-run one delivery from an exact completed release without promotion.
    Redeliver(crate::cli::release_submit::ReleaseRedeliverArgs),
    /// Manage the Stado-owned product and source policy catalog.
    Catalog(crate::cli::release_catalog::CatalogArgs),
    /// Manage product delivery destinations in the canonical registry.
    Destinations(super::destinations::DestinationArgs),
    /// Internal provider-neutral release build worker.
    #[command(hide = true)]
    Worker(crate::cli::release_submit::ReleaseWorkerArgs),
    /// Internal provider-neutral post-publication delivery worker.
    #[command(name = "delivery-worker", hide = true)]
    DeliveryWorker(crate::cli::release_submit::DeliveryWorkerArgs),
    /// Build, sign, and publish one immutable candidate coordinate.
    Prepare(ReleasePrepareArgs),
    /// Fetch signed, qualified archive bytes for one exact accepted source revision.
    Fetch(super::ReleaseFetchArgs),
    /// Promote exact qualified candidate bytes into registry desired state.
    Promote(ReleasePromoteArgs),
    /// Run one reconcile pass of desired releases on this exact registry
    /// target (`--once`); the resident reconciler is the
    /// `--release-interval-seconds` role of `stado serve`.
    Agent(ReleaseAgentArgs),
    /// Internal stable-port proxy owned by the release agent.
    #[command(hide = true)]
    Proxy(ReleaseProxyArgs),
    /// Show desired and observed rollout state, and the register of recent
    /// publication attempts.
    ///
    /// The newest pipeline runs are listed with each platform's job, its
    /// state, what the build cost from the job's own clock, and the recorded
    /// failure of anything that died - the builder that refused the work, the
    /// secret that could not be resolved, or the job's own last output - so a
    /// failed publication can be read back without opening the store, and a
    /// release that is getting slower can be seen without one. Stado Desktop
    /// shows the same register, the same durations and the same failure text
    /// on its Releases screen.
    ///
    /// An unfinished platform whose job is absent from every queue state is
    /// reported as failed, with its job id and the missing-job finding.
    /// Failure of a required platform also makes the observed run failed;
    /// recorded_state retains the stored run state. A queue-read failure is
    /// reported separately as job_read_error, not treated as a missing job.
    /// These are observations: status does not resubmit work or change the
    /// stored release state.
    Status(ReleaseStatusArgs),
    /// Resolve the executable this host runs for a product: the active signed
    /// release when release control rolls it out here, otherwise the version
    /// this host declares, checked against a fresh look at this host.
    #[command(name = "active-binary")]
    ActiveBinary(ReleaseActiveBinaryArgs),
    /// Print `<active.release_dir>/RELATIVE` from this host's release-state
    /// record of PRODUCT, or nothing when it records no active release.
    #[command(name = "active-dir", hide = true)]
    ActiveDir { product: String, relative: String },
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
    /// Reinstall a Stado release an earlier delivery retained on this very
    /// host, when the installed Stado cannot serve.
    #[command(name = "restore-local")]
    RestoreLocal(ReleaseRestoreLocalArgs),
    /// Reconcile live readers of an already-installed native binary.
    #[command(name = "converge-local-readers", hide = true)]
    ConvergeLocalReaders(ReleaseConvergeLocalReadersArgs),
    /// An immutable release coordinate: claim it for exactly one source
    /// revision before anything is published into it.
    #[command(subcommand)]
    Coordinate(ReleaseCoordinateCommands),
    /// A host's declared managed binary version: declare, unset or promote
    /// it, show it against what the host runs, or converge the host onto it.
    #[command(subcommand)]
    Version(ReleaseVersionCommands),
    /// A host's already-staged release.
    #[command(subcommand)]
    Staged(ReleaseStagedCommands),
    /// Attest the source and bytes of the release artifacts a host carries.
    Provenance(ReleaseProvenanceArgs),
    /// The pull-request version gate's steps: the advertised surface, the
    /// published baseline, the versioning rule and the module reachability
    /// check `.github/workflows/version-check.yml` runs.
    #[command(name = "version-gate", subcommand)]
    VersionGate(super::version_gate::VersionGateCommands),
}

/// One managed-version declaration of a registry host
/// (`targets[].managed_versions`).
#[derive(Subcommand)]
pub enum ReleaseVersionCommands {
    /// Declare the exact version a host must run of one binary.
    Declare(ReleaseDeclareVersionArgs),
    /// Remove one binary's declaration from a host.
    Unset(ReleaseUnsetVersionArgs),
    /// Verify one published version and promote it into a host's declaration.
    Promote(ReleasePromoteVersionArgs),
    /// Report the host's declared versions against what it runs.
    Show(ReleaseHostVersionArgs),
    /// Deliver host-behind versions; refuses to downgrade a host-ahead binary.
    Converge(ReleaseHostVersionArgs),
}

/// One immutable release coordinate.
#[derive(Subcommand)]
pub enum ReleaseCoordinateCommands {
    /// Bind one immutable coordinate to exactly one source revision before
    /// anything is published into it.
    Claim(ReleaseClaimCoordinateArgs),
}

/// One host's already-staged release.
#[derive(Subcommand)]
pub enum ReleaseStagedCommands {
    /// Activate one host's already-staged release with its own installer.
    Activate(ReleaseActivateStagedArgs),
}

/// Set one managed-version declaration for a registry host.
#[derive(Args)]
pub struct ReleaseDeclareVersionArgs {
    /// Registry target whose declaration is changed.
    #[arg(long)]
    host: String,
    #[arg(long)]
    binary: String,
    /// Exact version to declare.
    #[arg(long)]
    version: String,
    #[arg(long)]
    json: bool,
}

/// Remove one managed-version declaration from a registry host.
#[derive(Args)]
pub struct ReleaseUnsetVersionArgs {
    /// Registry target whose declaration is removed.
    #[arg(long)]
    host: String,
    #[arg(long)]
    binary: String,
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
    /// Product whose staged release is activated.
    #[arg(long)]
    product: String,
    /// Deployment env file declaring the staged coordinates.
    #[arg(long)]
    env_file: String,
    /// Port the activated release must answer on.
    #[arg(long)]
    port: u16,
    #[arg(long)]
    json: bool,
}

/// Read or converge one host against `targets[].managed_versions`.
#[derive(Args)]
pub struct ReleaseHostVersionArgs {
    #[arg(long)]
    host: String,
    /// Limit the report to one declared binary.
    #[arg(long)]
    binary: Option<String>,
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
    /// Remove this listener from the host process without stopping that process.
    #[arg(long)]
    stop: bool,
}
