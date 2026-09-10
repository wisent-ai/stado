//! Where the `stado host` verbs land: one dispatch per declaration block of
//! [`crate::cli::entry::spec::fleet::host`].

use crate::cli::entry::spec::fleet::host::runs::HostRunCommands;
use crate::cli::entry::spec::fleet::host::state::HostStateCommands;
use crate::cli::*;

pub(super) async fn dispatch(command: HostCommands) -> Result<(), CmdError> {
    match command {
        HostCommands::State(command) => state(command).await,
        HostCommands::Runs(command) => runs(command).await,
    }
}

async fn state(command: HostStateCommands) -> Result<(), CmdError> {
    match command {
        HostStateCommands::Health { target, json } => host::health(&target, json).await,
        HostStateCommands::PublishBeacon { source, print } => {
            host::publish_beacon(&source, print).await
        }
        HostStateCommands::BeaconUnits => host::beacon_units().await,
        HostStateCommands::CollectBeacon { publish } => host::collect_beacon(publish).await,
        HostStateCommands::Reboot { target } => host::reboot(&target).await,
        HostStateCommands::User(HostUserCommands::Create {
            username,
            target,
            all,
            full_name,
            shell,
            admin,
            require_password_change,
            dry_run,
            registry_source,
        }) => {
            host::user_create(
                &username,
                target,
                all,
                full_name,
                shell,
                admin,
                require_password_change,
                dry_run,
                &registry_source,
            )
            .await
        }
        HostStateCommands::User(HostUserCommands::Delete {
            username,
            target,
            keep_home,
        }) => host::user_delete(&username, &target, keep_home).await,
        HostStateCommands::GpuPowerLimit {
            target,
            watts,
            json,
        } => host::gpu_power_limit(&target, watts, json).await,
        HostStateCommands::Uptime { target, json } => host::uptime(&target, json).await,
        HostStateCommands::Ping { target, json } => host::ping(&target, json).await,
        HostStateCommands::Gates { host: target, json } => host::gates(&target, json).await,
        HostStateCommands::Link { target, json } => host::link(&target, json).await,
        HostStateCommands::UnitLog {
            target,
            unit,
            lines,
            json,
        } => host::unit_log(&target, &unit, lines, json).await,
        HostStateCommands::StorageRootReconcileWorker {
            target,
            target_config,
            transaction,
            phase,
            source_revision,
            tool_sha256,
            runner_gate,
        } => {
            host::storage_root_reconcile_worker(
                &target,
                &target_config,
                &transaction,
                &phase,
                &source_revision,
                &tool_sha256,
                &runner_gate,
            )
            .await
        }
    }
}

async fn runs(command: HostRunCommands) -> Result<(), CmdError> {
    match command {
        HostRunCommands::Cron {
            target,
            prune,
            restore,
            apply,
            json,
        } => host::cron(&target, prune.as_deref(), restore.as_deref(), apply, json).await,
        HostRunCommands::RenderSpisAdmissionTrust { target, source } => {
            host::render_spis_admission_trust(&target, &source).await
        }
        HostRunCommands::Exec {
            target,
            json,
            command,
        } => host::exec(&target, command, json).await,
        HostRunCommands::Deliver {
            target,
            source,
            destination,
            files_from,
            json,
        } => host::deliver(&target, &source, &destination, files_from.as_deref(), json).await,
        HostRunCommands::Build {
            target,
            manifest_path,
            binary,
            json,
        } => host::build(&target, &manifest_path, &binary, json).await,
        HostRunCommands::RunAttached {
            target,
            program,
            arguments,
            json,
        } => host::run_attached(&target, &program, &arguments, json).await,
        HostRunCommands::RemoveRunDirectory { target, path, json } => {
            host::remove_run_directory(&target, &path, json).await
        }
        HostRunCommands::Inventory { target, json } => host::inventory(&target, json).await,
        HostRunCommands::ConfigShow { target } => host::config_show(&target).await,
        HostRunCommands::ConfigSet {
            target,
            key,
            value,
            reload_service,
        } => host::config_set(&target, &key, &value, reload_service.as_deref()).await,
        HostRunCommands::ConfigUnset {
            target,
            key,
            reload_service,
        } => host::config_unset(&target, &key, reload_service.as_deref()).await,
    }
}
