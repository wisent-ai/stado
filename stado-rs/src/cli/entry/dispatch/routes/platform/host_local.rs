//! Where `stado host local` lands: the host-side halves of the host verbs,
//! declared in [`crate::cli::entry::spec::fleet::host::local`].

use crate::cli::entry::spec::fleet::host::local::{HostLocalCommands, HostLocalReconcileCommands};
use crate::cli::*;

pub(super) async fn dispatch(command: HostLocalCommands) -> Result<(), CmdError> {
    match command {
        HostLocalCommands::Run { lock, program } => host::run_locked(&lock, &program),
        HostLocalCommands::Api(command) => host::object_api_local(command).await,
        HostLocalCommands::Repair { config, product } => {
            host::release_store_repair_local(&config, &product)
        }
        HostLocalCommands::Audit {
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
                reclaim,
                apply: reclaim && apply,
            };
            if crate::deploy::host_backup_audit::local::run(&pass) {
                Ok(())
            } else {
                Err(crate::cli::CmdError::refused(
                    "the backup audit was refused before reading either store",
                ))
            }
        }
        HostLocalCommands::Port => {
            let listener =
                std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, u16::default()))
                    .map_err(|error| {
                        crate::cli::CmdError::click(format!(
                            "no free loopback port could be bound on this host: {error}"
                        ))
                        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                    })?;
            let port = listener
                .local_addr()
                .map_err(|error| {
                    crate::cli::CmdError::click(format!(
                        "the bound loopback port could not be read back: {error}"
                    ))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                })?
                .port();
            println!("{port}");
            Ok(())
        }
        HostLocalCommands::Reconcile(HostLocalReconcileCommands::Phase { phase, transaction }) => {
            crate::deploy::host_storage_reconcile_program::run(&phase, &transaction);
            Ok(())
        }
        HostLocalCommands::Reconcile(HostLocalReconcileCommands::Step(command)) => {
            crate::deploy::host_storage_reconcile_host::dispatch(command)
                .await
                .map_err(|refusal| {
                    eprintln!("{refusal}");
                    crate::cli::CmdError::silent(crate::cli::CLICK_ERROR_CODE)
                })
        }
        HostLocalCommands::Reconcile(HostLocalReconcileCommands::Worker {
            target,
            target_config,
            transaction,
            phase,
            source_revision,
            tool_sha256,
            runner_gate,
        }) => {
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
