//! Where the image, restart, release and delivery verbs land.

use super::*;

use crate::cli::service::lifecycle::release::gate::run::release;
use crate::cli::service::lifecycle::release::unit::{restart, stop};
use crate::cli::service::lifecycle::release::update::update;
use crate::cli::service::lifecycle::release::ServiceReleaseOptions;
use crate::cli::service::reports::view::show;
use crate::cli::service::runtime::files::fetch::{file_fetch, FileFetchOptions};
use crate::cli::service::runtime::files::sync::{file_sync, FileSyncOptions};
use crate::cli::service::runtime::secrets::sync::{secret_sync, SecretSyncOptions};

use super::super::spec::runtime::RuntimeCommands;

pub(crate) async fn dispatch(command: RuntimeCommands) -> Result<(), CmdError> {
    match command {
        RuntimeCommands::RefreshImage {
            name,
            if_needed,
            json,
        } => crate::cli::service_refresh_image::refresh_image(&name, if_needed, json).await,
        RuntimeCommands::Update {
            name,
            host,
            from_artifact,
            from_archive,
            rollback_to,
            refresh_image,
            json,
        } => {
            update(
                &name,
                &host,
                from_artifact.as_deref(),
                from_archive.as_deref(),
                rollback_to.as_deref(),
                refresh_image,
                json,
            )
            .await
        }
        RuntimeCommands::Release {
            name,
            host,
            product,
            version,
            readiness_url,
            readiness_timeout_seconds,
            reload_unit,
            require_release_version,
            supersede_unit,
            json,
        } => {
            release(ServiceReleaseOptions {
                name: &name,
                host: &host,
                product: &product,
                version: &version,
                readiness_url: readiness_url.as_deref(),
                readiness_timeout_seconds,
                reload_unit,
                require_release_version,
                supersede_unit: supersede_unit.as_deref(),
                supersede_same_label_user: false,
                json,
                emit: true,
            })
            .await
        }
        RuntimeCommands::Show { name, host, json } => show(&name, host.as_deref(), json).await,
        RuntimeCommands::RepairRunnerRuntime { name, host, json } => {
            let services = declared_matching(&name, Some(&host)).await?;
            let target = host_channel::canonical_target(&host).await.map_err(click)?;
            let runner = production_runner();
            for managed in &services {
                let report =
                    crate::deploy::host_precheck_runner::repair_runtime(&target, managed, &runner)
                        .await
                        .map_err(click)?;
                if json {
                    print_json(&report)?;
                } else {
                    print!("{}", report["stdout"].as_str().unwrap_or_default());
                }
            }
            Ok(())
        }
        RuntimeCommands::Stop {
            name,
            host,
            listener_url,
            json,
        } => stop(&name, host.as_deref(), listener_url.as_deref(), json).await,
        RuntimeCommands::Restart {
            name,
            host,
            take_over_listener,
            recovery_unit,
            json,
        } => {
            restart(
                &name,
                host.as_deref(),
                take_over_listener.as_deref(),
                recovery_unit.as_deref(),
                json,
            )
            .await
        }
        RuntimeCommands::SecretSync {
            name,
            host,
            item,
            field,
            variable,
            env_file,
            restart,
            json,
        } => {
            secret_sync(SecretSyncOptions {
                name: &name,
                host: &host,
                item: &item,
                field: &field,
                variable: &variable,
                env_file: &env_file,
                restart_after_sync: restart,
                as_json: json,
            })
            .await
        }
        RuntimeCommands::FileSync {
            name,
            host,
            source_file,
            target_file,
            executable,
            json,
        } => {
            file_sync(FileSyncOptions {
                name: &name,
                host: &host,
                source_file: &source_file,
                target_file: &target_file,
                executable,
                as_json: json,
            })
            .await
        }
        RuntimeCommands::FileFetch {
            name,
            host,
            source_file,
            dest_file,
            json,
        } => {
            file_fetch(FileFetchOptions {
                name: &name,
                host: &host,
                source_file: &source_file,
                dest_file: dest_file.as_deref(),
                as_json: json,
            })
            .await
        }
    }
}
