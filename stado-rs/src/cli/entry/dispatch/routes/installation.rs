//! Where the installation, account and local-upkeep verbs land.

use crate::cli::entry::spec::root::installation::InstallationCommands;
use crate::cli::*;

pub(crate) async fn dispatch(command: InstallationCommands) -> Result<(), CmdError> {
    match command {
        InstallationCommands::PackageRoot => {
            // Python prints the installed package source root; the Rust
            // equivalent is the crate data directory (profiles, templates,
            // registry) used by desktop provisioning.
            println!("{}", crate::data_dir().display());
            Ok(())
        }
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
        InstallationCommands::Optimize(command) => autonomy_cmd::dispatch_optimize(command).await,
        InstallationCommands::Billing(sub) => billing::dispatch(&sub).await,
        InstallationCommands::Azure(sub) => azure::dispatch(sub).await,
        InstallationCommands::Cloudflare(sub) => cloudflare::dispatch(sub).await,
        InstallationCommands::Mail(sub) => mail::dispatch(&sub).await,
        InstallationCommands::DiskCleanup {
            once,
            watch,
            to_target,
            dry_run,
        } => disk_cleanup::run(once, watch, to_target, dry_run).await,
        InstallationCommands::InstallDiskCleanup => disk_cleanup::install().await,
        InstallationCommands::Workdirs { apply, json } => workdirs::run(apply, json),
    }
}
