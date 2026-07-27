//! Provider-neutral inventory, planning, execution, verification, and restore.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use super::CmdError;

pub mod engine;
pub mod executors;
pub mod inventory;
pub mod journal;
pub mod model;
pub mod planner;
pub mod rationalize;
pub mod shutdown;

#[derive(Subcommand, Debug)]
pub enum ResourcesCommands {
    /// Show one read-only inventory across configured providers and storage.
    Show(ShowArgs),
    /// Produce an immutable resource rationalization plan; never mutates.
    Rationalize(RationalizeArgs),
    /// Execute the exact rationalization plan supplied by the operator.
    #[command(name = "kill-irrational")]
    KillIrrational(KillIrrationalArgs),
    /// Produce a reversible shutdown plan; never mutates.
    Shutdown(ShutdownArgs),
    /// Execute the exact shutdown plan supplied by the operator.
    Apply(ApplyArgs),
    /// Verify live resources against an applied or restored operation.
    Verify(VerifyArgs),
    /// Restore successfully applied reversible actions.
    Restore(RestoreArgs),
    /// Inspect durable operation history.
    #[command(subcommand)]
    Operations(OperationsCommands),
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Restrict cloud inventory to one provider.
    #[arg(long)]
    pub provider: Option<String>,
    /// Emit the versioned machine-readable inventory.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RationalizeArgs {
    /// Restrict recommendations to one provider.
    #[arg(long)]
    pub provider: Option<String>,
    /// Ignore candidates younger than this (`30m`, `24h`, `7d`).
    #[arg(long, default_value = "24h")]
    pub min_age: String,
    /// Write the immutable canonical plan here.
    #[arg(long)]
    pub output: PathBuf,
    /// Emit the versioned machine-readable summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct KillIrrationalArgs {
    /// Canonical plan produced by `resources rationalize`.
    #[arg(long)]
    pub plan: PathBuf,
    /// Exact SHA-256 printed when the plan was created.
    #[arg(long)]
    pub expect_hash: String,
    /// Explicitly approve one review-required action. Repeatable.
    #[arg(long)]
    pub approve: Vec<String>,
    /// Required acknowledgement after reviewing the plan.
    #[arg(long)]
    pub yes: bool,
    /// Required when any selected action is irreversible.
    #[arg(long)]
    pub allow_irreversible: bool,
    /// Emit a machine-readable execution summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ShutdownArgs {
    /// Exact GCP project id used for discovery and every locator.
    #[arg(long)]
    pub project: String,
    /// Discover resources whose Stado ownership is authoritative.
    #[arg(long, conflicts_with = "resource")]
    pub all_stado_owned: bool,
    /// Exact typed locator, for example gcp:instance:ZONE/NAME. Repeatable.
    #[arg(long, conflicts_with = "all_stado_owned")]
    pub resource: Vec<String>,
    /// Write the immutable canonical plan here.
    #[arg(long)]
    pub output: PathBuf,
    /// Emit a machine-readable summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// Canonical shutdown plan produced by `resources shutdown`.
    #[arg(long)]
    pub plan: PathBuf,
    /// Exact SHA-256 printed when the plan was created.
    #[arg(long)]
    pub expect_hash: String,
    /// Required acknowledgement after reviewing the plan.
    #[arg(long)]
    pub yes: bool,
    /// Emit a machine-readable execution summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct VerifyArgs {
    /// Durable operation id.
    #[arg(long)]
    pub operation: String,
    /// Emit a machine-readable verification report.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RestoreArgs {
    /// Durable operation id.
    #[arg(long)]
    pub operation: String,
    /// Required acknowledgement after reviewing rollback coverage.
    #[arg(long)]
    pub yes: bool,
    /// Emit a machine-readable execution summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum OperationsCommands {
    /// List durable operations newest first.
    List,
    /// Show the archived plan, state, and events for one operation.
    Show { operation_id: String },
}

pub async fn dispatch(command: ResourcesCommands) -> Result<(), CmdError> {
    match command {
        ResourcesCommands::Show(args) => inventory::run(&args).await,
        ResourcesCommands::Rationalize(args) => rationalize::run(&args).await,
        ResourcesCommands::KillIrrational(args) => engine::kill_irrational(&args).await,
        ResourcesCommands::Shutdown(args) => shutdown::run(&args).await,
        ResourcesCommands::Apply(args) => engine::apply_shutdown(&args).await,
        ResourcesCommands::Verify(args) => engine::verify(&args).await,
        ResourcesCommands::Restore(args) => engine::restore(&args).await,
        ResourcesCommands::Operations(command) => journal::dispatch(command).await,
    }
}
