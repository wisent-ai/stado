//! One match from a parsed subcommand to the verb that owns it.

use crate::cli::release_cmd::local::converge::converge_local_readers;
use crate::cli::release_cmd::local::install::install_local;
use crate::cli::release_cmd::local::restore::restore_local;
use crate::cli::release_cmd::publication::claims::claim_coordinate;
use crate::cli::release_cmd::publication::signing::{keygen, prepare};
use crate::cli::release_cmd::rollout::promote::promote;
use crate::cli::release_cmd::rollout::reconcile::{active_binary, agent, rollback};
use crate::cli::release_cmd::rollout::status::status;
use crate::cli::CmdError;

use super::{
    ReleaseCommands, ReleaseCoordinateCommands, ReleaseStagedCommands, ReleaseVersionCommands,
};

pub async fn dispatch(command: ReleaseCommands) -> Result<(), CmdError> {
    match command {
        ReleaseCommands::Keygen(args) => keygen(&args).await,
        ReleaseCommands::Policy(command) => {
            crate::cli::release_cmd::rollout::policy::run(&command).await
        }
        ReleaseCommands::Submit(args) => crate::cli::release_submit::submit(&args).await,
        ReleaseCommands::Newest(args) => crate::cli::release_newest::newest(&args).await,
        ReleaseCommands::Changes(args) => {
            crate::cli::release_submit::changes::dispatch(&args).await
        }
        ReleaseCommands::Resume(args) => crate::cli::release_submit::resume(&args).await,
        ReleaseCommands::Redeliver(args) => crate::cli::release_submit::redeliver(&args).await,
        ReleaseCommands::Catalog(args) => crate::cli::release_catalog::dispatch(args).await,
        ReleaseCommands::Destinations(args) => super::super::destinations::dispatch(args).await,
        ReleaseCommands::Worker(args) => crate::cli::release_submit::worker(&args).await,
        ReleaseCommands::DeliveryWorker(args) => {
            crate::cli::release_submit::delivery_worker(&args).await
        }
        ReleaseCommands::Prepare(args) => prepare(&args).await,
        ReleaseCommands::Fetch(args) => super::super::fetch::fetch(&args).await,
        ReleaseCommands::Promote(args) => promote(&args, false).await,
        ReleaseCommands::Agent(args) => agent(&args).await,
        ReleaseCommands::Proxy(args) => {
            if args.stop {
                crate::release_agent::rollout::serving::control::stop(None, &args.state, &args.bind)
                    .await
                    .map_err(CmdError::from)
            } else {
                crate::release_agent::proxy(&args.state, &args.bind)
                    .await
                    .map_err(CmdError::from)
            }
        }
        ReleaseCommands::Status(args) => status(&args).await,
        ReleaseCommands::ActiveBinary(args) => active_binary(&args).await,
        ReleaseCommands::ActiveDir { product, relative } => {
            active_dir(&product, &relative);
            Ok(())
        }
        ReleaseCommands::Logs(args) => crate::cli::release_evidence::dispatch_logs(&args).await,
        ReleaseCommands::Doctor(args) => crate::cli::release_evidence::dispatch_doctor(&args).await,
        ReleaseCommands::Quarantine(sub) => crate::cli::release_quarantine::dispatch(sub).await,
        ReleaseCommands::Rollback(args) => rollback(&args).await,
        ReleaseCommands::InstallLocal(args) => install_local(&args).await,
        ReleaseCommands::RestoreLocal(args) => restore_local(&args).await,
        ReleaseCommands::ConvergeLocalReaders(args) => converge_local_readers(&args).await,
        ReleaseCommands::Coordinate(ReleaseCoordinateCommands::Claim(args)) => {
            claim_coordinate(&args).await
        }
        ReleaseCommands::Version(command) => version(command).await,
        ReleaseCommands::Staged(ReleaseStagedCommands::Activate(args)) => {
            crate::cli::host::activate_staged_release(
                &args.host,
                &args.product,
                &args.env_file,
                args.port,
                args.json,
            )
            .await
        }
        ReleaseCommands::Provenance(args) => {
            crate::cli::host::provenance(&args.host, args.json).await
        }
        ReleaseCommands::VersionGate(command) => super::super::version_gate::dispatch(command),
    }
}

/// The verbs on one host's declared managed version.
async fn version(command: ReleaseVersionCommands) -> Result<(), CmdError> {
    match command {
        ReleaseVersionCommands::Declare(args) => {
            crate::cli::host::declare_version(
                &args.host,
                &args.binary,
                Some(&args.version),
                false,
                args.json,
            )
            .await
        }
        ReleaseVersionCommands::Unset(args) => {
            crate::cli::host::declare_version(&args.host, &args.binary, None, true, args.json).await
        }
        ReleaseVersionCommands::Promote(args) => {
            crate::cli::host::promote_version(&args.host, &args.binary, &args.version, args.json)
                .await
        }
        ReleaseVersionCommands::Show(args) => {
            crate::cli::service_converge::converge(
                &args.host,
                args.binary.as_deref(),
                false,
                args.json,
            )
            .await
        }
        ReleaseVersionCommands::Converge(args) => {
            crate::cli::service_converge::converge(
                &args.host,
                args.binary.as_deref(),
                true,
                args.json,
            )
            .await
        }
    }
}

/// The release agent's own record of the active release directory; a missing
/// or unreadable record prints nothing, which callers read as "not recorded".
fn active_dir(product: &str, relative: &str) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let state = std::path::Path::new(&home)
        .join(".stado/release-state")
        .join(format!("{product}.json"));
    let directory = std::fs::read(&state)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|state| {
            state
                .pointer("/active/release_dir")?
                .as_str()
                .map(str::to_string)
        })
        .filter(|directory| !directory.is_empty());
    if let Some(directory) = directory {
        println!("{directory}/{relative}");
    }
}
