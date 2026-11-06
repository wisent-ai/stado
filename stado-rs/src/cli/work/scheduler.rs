//! `stado scheduler tick`: one coordinator tick, now, in this process.
//!
//! The coordinator runs the tick on its own cadence (`stado serve
//! --coordinator`, `stado serve --control-plane`); this verb runs the same
//! [`crate::coordinator::run_tick`] once, for an operator who wants a queued
//! job dispatched, a due schedule fired or a dead agent reaped without waiting
//! for the next interval, and for a journey that has to observe one tick end
//! to end. Nothing here is a second scheduler: the passes, their order and
//! their refusals are the coordinator's own.

use clap::{Args, Subcommand};
use serde_json::json;

use crate::coordinator::ResolvedProvider;
use crate::queue::JobStorage;

use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum SchedulerCommands {
    /// Run one coordinator tick now: settle cancellations, fire due
    /// schedules, reap expired leases, assign, then check, reap and dispatch
    /// for each provider arm, and reap terminal runs.
    Tick(TickArgs),
}

#[derive(Args)]
pub struct TickArgs {
    /// Run the arm of this provider only (repeatable); each must be enabled
    /// in the profile and have a tick arm. Without it, every arm of
    /// `providers` (WC_PROVIDERS) runs, as the coordinator's tick does.
    #[arg(long = "provider")]
    providers: Vec<String>,
    /// Also run the billing collector at the end of the tick, as the cloud
    /// coordinator's tick does; the local control plane's tick does not.
    #[arg(long)]
    billing: bool,
    /// Print the answer as JSON.
    #[arg(long)]
    json: bool,
}

pub async fn dispatch(command: SchedulerCommands) -> Result<(), CmdError> {
    match command {
        SchedulerCommands::Tick(args) => tick(args).await,
    }
}

/// The arm's provider as `WC_PROVIDERS` names it.
fn arm_name(arm: &ResolvedProvider) -> &str {
    match arm {
        ResolvedProvider::Cloud { name, .. } | ResolvedProvider::Box { name, .. } => name,
    }
}

/// The arms to run: every resolved arm, or the named ones, each refused by
/// name when it is not a compute provider, is fenced or not enabled, or
/// resolves to no arm here.
fn selected_arms(named: &[String]) -> Result<Vec<ResolvedProvider>, CmdError> {
    let arms = crate::coordinator::resolve_providers();
    if named.is_empty() {
        return Ok(arms);
    }
    let mut wanted = Vec::with_capacity(named.len());
    for name in named {
        let Some(variant) =
            crate::capabilities::variant(crate::capabilities::RuntimeFacet::Compute, name.trim())
        else {
            return Err(CmdError::usage(format!(
                "--provider {name:?} is not a compute provider of the capability catalog; \
                 `stado capabilities compute` lists them"
            )));
        };
        crate::providers::require_enabled(variant.id)?;
        if !arms.iter().any(|arm| arm_name(arm) == variant.id) {
            return Err(CmdError::refused(format!(
                "provider {} has no tick arm on this deployment: a registered host claims its own \
                 work, and a provider whose client could not be built says why on the [tick] line \
                 above",
                variant.id
            )));
        }
        wanted.push(variant.id);
    }
    Ok(arms
        .into_iter()
        .filter(|arm| wanted.contains(&arm_name(arm)))
        .collect())
}

async fn tick(args: TickArgs) -> Result<(), CmdError> {
    let arms = selected_arms(&args.providers)?;
    let store = JobStorage::new().await?;
    // The scoped workload grant agent dispatch projects into the machines it
    // rents, resolved as the coordinator resolves it before its first tick.
    let secrets = crate::coordinator::secrets_from_skarbiec().await?;
    let scheduled = crate::coordinator::run_tick(&store, &secrets, &arms, args.billing, &|line| {
        eprintln!("[tick] {line}")
    })
    .await?;
    crate::cli::print_answer(
        &json!({
            "scheduled": scheduled,
            "providers": arms.iter().map(arm_name).collect::<Vec<_>>(),
            "billing": args.billing,
        }),
        args.json,
    )
}
