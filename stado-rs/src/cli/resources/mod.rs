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
    /// Explicitly adopt one observed resource into Stado ownership.
    Adopt(AdoptArgs),
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
pub struct AdoptArgs {
    /// Canonical `stado://` resource id from `resources show`.
    pub resource_id: String,
    /// Accountable owner recorded on every autonomous action.
    #[arg(long)]
    pub owner: String,
    /// Versioned autonomy policy governing the adopted resource.
    #[arg(long)]
    pub policy_ref: String,
    /// Exact provider revision observed during review.
    #[arg(long)]
    pub expect_revision: String,
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
        ResourcesCommands::Adopt(args) => adopt(&args).await,
        ResourcesCommands::Rationalize(args) => rationalize::run(&args).await,
        ResourcesCommands::KillIrrational(args) => engine::kill_irrational(&args).await,
        ResourcesCommands::Shutdown(args) => shutdown::run(&args).await,
        ResourcesCommands::Apply(args) => engine::apply_shutdown(&args).await,
        ResourcesCommands::Verify(args) => engine::verify(&args).await,
        ResourcesCommands::Restore(args) => engine::restore(&args).await,
        ResourcesCommands::Operations(command) => journal::dispatch(command).await,
    }
}
async fn adopt(args: &AdoptArgs) -> Result<(), CmdError> {
    let store = crate::queue::JobStorage::new().await?;
    let inventory = crate::autonomy::storage::load_latest_inventory(&store)
        .await?
        .ok_or_else(|| {
            CmdError::click("no autonomy inventory snapshot; run `stado optimize run`")
        })?;
    let resource = inventory
        .resources
        .iter()
        .find(|resource| resource.resource_id == args.resource_id)
        .ok_or_else(|| CmdError::click(format!("resource not found: {}", args.resource_id)))?;
    if resource.source_revision.as_deref() != Some(args.expect_revision.as_str()) {
        return Err(CmdError::click(
            "resource revision changed or is unavailable; refresh inventory and review again",
        ));
    }
    if matches!(
        resource.ownership,
        crate::autonomy::model::Ownership::Owned | crate::autonomy::model::Ownership::Adopted
    ) {
        return Err(CmdError::click("resource is already owned or adopted"));
    }
    let adoption = crate::autonomy::model::AdoptionRecord {
        schema_version: crate::autonomy::model::SCHEMA_VERSION,
        resource_id: resource.resource_id.clone(),
        adopted_at: chrono::Utc::now().to_rfc3339(),
        adopted_by: std::env::var("USER").unwrap_or_else(|_| "operator".to_string()),
        owner: args.owner.clone(),
        policy_ref: args.policy_ref.clone(),
        source_revision: resource.source_revision.clone(),
    };
    crate::autonomy::storage::write_adoption(&store, &adoption).await?;
    println!(
        "Adopted {} at revision {}",
        adoption.resource_id, args.expect_revision
    );
    Ok(())
}
