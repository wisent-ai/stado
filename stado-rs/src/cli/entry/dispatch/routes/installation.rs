//! Where the installation, account and local-upkeep verbs land.

use crate::cli::entry::spec::root::installation::InstallationCommands;
use crate::cli::hosts::disk_cleanup;
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
        InstallationCommands::Capabilities { capability, json } => {
            capabilities::run(capability.as_deref(), json)
        }
        InstallationCommands::Overview { json } => overview::run(json).await,
        InstallationCommands::BlastRadius(args) => blast_radius::run(&args).await,
        InstallationCommands::Resources(command) => resources::dispatch(command).await,
        InstallationCommands::Optimize(command) => autonomy::dispatch_optimize(command).await,
        InstallationCommands::Billing(sub) => billing::dispatch(&sub).await,
        InstallationCommands::Cloud(sub) => azure::dispatch(sub).await,
        InstallationCommands::Tunnel(sub) => cloudflare::dispatch(sub).await,
        InstallationCommands::DiskCleanup { dry_run } => disk_cleanup::run(dry_run).await,
        InstallationCommands::Workdirs {
            apply,
            include_files,
            json,
        } => workdirs::run(apply, include_files, json),
    }
}
