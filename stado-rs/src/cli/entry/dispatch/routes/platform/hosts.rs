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
        HostStateCommands::CollectBeacon { publish } => host::collect_beacon(publish).await,
        HostStateCommands::Privacy { target, json, open } => {
            host::privacy(&target, json, open).await
        }
        HostStateCommands::Reboot { target } => host::reboot(&target).await,
        HostStateCommands::DiskCleanup {
            target,
            dry_run,
            json,
        } => host::disk_cleanup(&target, dry_run, json).await,
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
            json,
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
                registry_source,
                json,
            )
            .await
        }
        HostStateCommands::User(HostUserCommands::Delete {
            username,
            target,
            keep_home,
            confirm,
            json,
        }) => host::user_delete(&username, &target, keep_home, &confirm, json).await,
        HostStateCommands::GpuPowerLimit {
            target,
            watts,
            json,
        } => host::gpu_power_limit(&target, watts, json).await,
        HostStateCommands::GpuPowerLimitUnset { target, json } => {
            host::gpu_power_limit_unset(&target, json).await
        }
        HostStateCommands::Uptime { target, json } => host::uptime(&target, json).await,
        HostStateCommands::Ping { target, json } => host::ping(&target, json).await,
        HostStateCommands::Gates {
            host: target,
            json,
            require_disk,
        } => host::gates(&target, json, require_disk).await,
        HostStateCommands::Link { target, json } => host::link(&target, json).await,
        HostStateCommands::UnitLog {
            target,
            unit,
            lines,
            json,
        } => host::unit_log(&target, &unit, lines, json).await,
        HostStateCommands::PortOwner { target, port, json } => {
            host::port_owner(&target, port, json).await
        }
        HostStateCommands::RunLocked { lock, program } => host::run_locked(&lock, &program),
        HostStateCommands::ObjectApiLocal(command) => host::object_api_local(command).await,
        HostStateCommands::ReleaseStoreRepairLocal { config, product } => {
            host::release_store_repair_local(&config, &product)
        }
        HostStateCommands::BackupAuditLocal {
            backup,
            primary,
            namespace,
            reclaim,
            apply,
            objects_hex,
            inventory_namespaces_hex,
        } => {
            let pass = crate::deploy::host_backup_audit::local::LocalPass {
                backup,
                primary,
                namespace,
                objects: crate::deploy::host_backup_audit::local::hex_list(&objects_hex)
                    .map_err(crate::cli::CmdError::usage)?,
                inventory_namespaces: crate::deploy::host_backup_audit::local::hex_list(
                    &inventory_namespaces_hex,
                )
                .map_err(crate::cli::CmdError::usage)?,
                reclaim: reclaim == "yes",
                apply: reclaim == "yes" && apply == "yes",
            };
            if crate::deploy::host_backup_audit::local::run(&pass) {
                Ok(())
            } else {
                Err(crate::cli::CmdError::refused(
                    "the backup audit was refused before reading either store",
                ))
            }
        }
        HostStateCommands::FreePortLocal => {
            let listener =
                std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, u16::default()))
                    .map_err(|error| {
                        crate::cli::CmdError::click(format!(
                            "no free loopback port could be bound on this host: {error}"
                        ))
                    })?;
            let port = listener
                .local_addr()
                .map_err(|error| {
                    crate::cli::CmdError::click(format!(
                        "the bound loopback port could not be read back: {error}"
                    ))
                })?
                .port();
            println!("{port}");
            Ok(())
        }
        HostStateCommands::StorageRootReconcileLocal { phase, transaction } => {
            crate::deploy::host_storage_reconcile_program::run(&phase, &transaction);
            Ok(())
        }
        HostStateCommands::StorageRootReconcileHost(command) => {
            crate::deploy::host_storage_reconcile_host::dispatch(command)
                .await
                .map_err(|refusal| {
                    eprintln!("{refusal}");
                    crate::cli::CmdError::silent(crate::cli::CLICK_ERROR_CODE)
                })
        }
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
        HostRunCommands::RenderPublicDocument { target, source } => {
            host::render_public_document(&target, &source).await
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
        HostRunCommands::CompilerCache {
            target,
            operation,
            json,
        } => host::compiler_cache(&target, operation, json).await,
        HostRunCommands::ConfigShow { target, json } => host::config_show(&target, json).await,
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
