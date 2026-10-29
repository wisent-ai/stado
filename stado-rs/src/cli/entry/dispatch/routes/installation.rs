//! Where the installation, account and local-upkeep verbs land.

use crate::cli::entry::spec::root::installation::{CapabilityAction, InstallationCommands};
use crate::cli::setup::onboarding;
use crate::cli::work::autonomy;
use crate::cli::*;

pub(crate) async fn dispatch(command: InstallationCommands) -> Result<(), CmdError> {
    match command {
        InstallationCommands::Onboarding {
            reset,
            import_registry,
            json,
        } => onboarding::run(reset, import_registry, json).await,
        InstallationCommands::Capabilities {
            capability,
            json,
            action,
        } => match action {
            None => capabilities::run(capability.as_deref(), json),
            Some(CapabilityAction::Resolve {
                family,
                consumer,
                failed,
                json,
            }) => capabilities::resolve(&family, &consumer, &failed, json).await,
            Some(CapabilityAction::Select { family, order }) => {
                capabilities::select(&family, &order)
            }
        },
        InstallationCommands::Errors(command) => crate::cli::errors::dispatch(command),
        InstallationCommands::Overview { json } => overview::run(json).await,
        InstallationCommands::BlastRadius(args) => blast_radius::run(&args).await,
        InstallationCommands::Resources(command) => resources::dispatch(command).await,
        InstallationCommands::Optimize(command) => autonomy::dispatch_optimize(command).await,
        InstallationCommands::Billing(sub) => billing::dispatch(&sub).await,
        InstallationCommands::Cloud(sub) => azure::dispatch(sub).await,
        InstallationCommands::Tunnel(sub) => cloudflare::dispatch(sub).await,
        InstallationCommands::Waits { action, json } => waits::dispatch(action, json),
        InstallationCommands::PublicStatus(command) => public_status::dispatch(command).await,
    }
}
