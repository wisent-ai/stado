//! `stado recovery migrate` — fenced, provider-neutral storage cutover.
//!
//! The command deliberately keeps the queue paused at every failure boundary.
//! It opens an optional GCP billing window only around source reads, closes it
//! before any workload is resumed, and switches only explicitly named services
//! and compute providers.
//!
//! # The components
//!
//! [`request`] is the parsed command: the argument types, the up-front
//! validation that runs before anything is touched, and the dry-run plan.
//! [`fence`] pauses and drains one store and stops or restarts the declared
//! services. [`billing`] owns the bounded Cloud Billing window and nothing
//! else. [`cutover`] prepares the destination config and installs it locally
//! and on every resolved host.
//!
//! The nine numbered steps stay here, in [`migrate`] and [`transfer`], because
//! the order of the fences — and the failure boundary between each pair of
//! them — is the guarantee this command makes.

mod billing;
mod cutover;
mod fence;
mod request;

use clap::Subcommand;

use crate::cli::recovery::billing::{
    close_billing_window, combine_billing_error, ensure_billing_disabled, update_gcp_billing,
};
use crate::cli::recovery::cutover::install::{install_local_config, install_remote_configs};
use crate::cli::recovery::cutover::prepare_config;
use crate::cli::recovery::fence::services::{resolve_services, restart_activated, stop_services};
use crate::cli::recovery::fence::{drain_store, endpoint_store};
use crate::cli::recovery::request::dry_run::print_plan;
use crate::cli::recovery::request::validate::{required, validate_args};
use crate::cli::recovery::request::ResolvedService;
use crate::cli::{storage, CmdError};
use crate::deploy::DeployError;
use crate::queue::control;
use crate::queue::copy::{CopyOptions, Endpoint};

pub use crate::cli::recovery::request::RecoveryMigrateArgs;

const FENCE_REASON: &str = "fenced by stado recovery migrate";

#[derive(Subcommand, Debug)]
pub enum RecoveryCommands {
    /// Drain, copy, verify, cut over selected services, and optionally resume.
    Migrate(Box<RecoveryMigrateArgs>),
}

pub async fn dispatch(command: RecoveryCommands) -> Result<(), CmdError> {
    match command {
        RecoveryCommands::Migrate(args) => migrate(&args).await,
    }
}

async fn migrate(args: &RecoveryMigrateArgs) -> Result<(), CmdError> {
    let source = args.ends.source();
    let destination = args.ends.destination();
    validate_args(args, &source, &destination)?;
    let prepared = prepare_config(args, &destination)?;
    if args.dry_run {
        print_plan(args, &source, &destination, &prepared);
        return Ok(());
    }

    let services = resolve_services(args).await?;
    println!("[1/9] fencing destination {}", destination.describe());
    let destination_store = endpoint_store(&destination).await?;
    control::set_paused(&destination_store, true, FENCE_REASON, "").await?;
    drain_store(
        &destination_store,
        &destination.describe(),
        args.drain_timeout,
    )
    .await?;

    let mut billing_attempted = false;
    if args.manage_gcp_billing {
        let project = required(args.gcp_project.as_deref(), "--gcp-project")?;
        let account = required(args.gcp_billing_account.as_deref(), "--gcp-billing-account")?;
        println!("[2/9] opening bounded GCP billing window for {project}");
        ensure_billing_disabled(project).await?;
        billing_attempted = true;
        if let Err(open_error) = update_gcp_billing(project, account).await {
            let close = close_billing_window(project).await;
            return Err(combine_billing_error(open_error, close));
        }
    } else {
        println!("[2/9] using source without a managed billing window");
    }

    let transfer_result = transfer(args, &source, &destination, &services).await;
    let close_result = if billing_attempted {
        let project = required(args.gcp_project.as_deref(), "--gcp-project")?;
        println!("[6/9] closing GCP billing window before cutover");
        close_billing_window(project).await
    } else {
        Ok(())
    };
    match (transfer_result, close_result) {
        (Err(transfer), Err(close)) => return Err(CmdError::click(format!("{transfer}; CRITICAL: transfer failed and the GCP billing window could not be closed: {close}. Both stores remain PAUSED"))),
        (Err(transfer), Ok(())) => return Err(transfer),
        (Ok(()), Err(close)) => return Err(CmdError::click(format!("CRITICAL: verified transfer completed, but the GCP billing window could not be closed: {close}. Cutover was not started and both stores remain PAUSED"))),
        (Ok(()), Ok(())) => {}
    }

    println!(
        "[7/9] atomically switching Stado config to {}",
        destination.describe()
    );
    install_local_config(&prepared)?;
    install_remote_configs(&services, &prepared.bytes).await?;
    println!("[8/9] restarting only explicitly activated services");
    restart_activated(&services).await?;
    if args.resume {
        println!(
            "[9/9] resuming dispatch and claims on {}",
            destination.describe()
        );
        control::set_paused(&destination_store, false, "", "").await?;
    } else {
        println!("[9/9] destination remains PAUSED (no --resume)");
    }
    println!(
        "recovery migration complete: {} -> {}; GCP compute is absent from the provider allowlist",
        source.describe(),
        destination.describe()
    );
    Ok(())
}

async fn transfer(
    args: &RecoveryMigrateArgs,
    source: &Endpoint,
    destination: &Endpoint,
    services: &[ResolvedService],
) -> Result<(), CmdError> {
    println!("[3/9] fencing and draining source {}", source.describe());
    let source_store = endpoint_store(source).await?;
    control::set_paused(&source_store, true, FENCE_REASON, "").await?;
    drain_store(&source_store, &source.describe(), args.drain_timeout).await?;
    println!("[4/9] stopping every declared writer before the final copy");
    stop_services(services).await?;
    println!("[5/9] copying the complete canonical namespace");
    storage::copy_between(
        source.clone(),
        destination.clone(),
        CopyOptions {
            prefixes: Vec::new(),
            concurrency: args.concurrency.get(),
        },
        false,
        false,
    )
    .await?;
    println!("[5/9] verifying names, metadata, and body bytes read-only");
    storage::verify_between(source.clone(), destination.clone(), &[], false).await?;
    Ok(())
}

fn deploy_error(error: DeployError) -> CmdError {
    CmdError::click(error.to_string())
}
