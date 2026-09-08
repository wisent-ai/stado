//! One match from a parsed subcommand to the verb that owns it.

use crate::cli::release_cmd::local::converge::converge_local_readers;
use crate::cli::release_cmd::local::install::install_local;
use crate::cli::release_cmd::publication::claims::claim_coordinate;
use crate::cli::release_cmd::publication::signing::{keygen, prepare};
use crate::cli::release_cmd::rollout::policy::apply_policy;
use crate::cli::release_cmd::rollout::promote::promote;
use crate::cli::release_cmd::rollout::reconcile::{active_binary, agent, rollback};
use crate::cli::release_cmd::rollout::status::status;
use crate::cli::CmdError;

use super::ReleaseCommands;

pub async fn dispatch(command: ReleaseCommands) -> Result<(), CmdError> {
    match command {
        ReleaseCommands::Keygen(args) => keygen(&args).await,
        ReleaseCommands::PolicyApply(args) => apply_policy(&args).await,
        ReleaseCommands::Submit(args) => crate::cli::release_submit::submit(&args).await,
        ReleaseCommands::Redeliver(args) => crate::cli::release_submit::redeliver(&args).await,
        ReleaseCommands::Catalog(args) => crate::cli::release_catalog::dispatch(args).await,
        ReleaseCommands::Worker(args) => crate::cli::release_submit::worker(&args).await,
        ReleaseCommands::DeliveryWorker(args) => {
            crate::cli::release_submit::delivery_worker(&args).await
        }
        ReleaseCommands::Prepare(args) => prepare(&args).await,
        ReleaseCommands::Promote(args) => promote(&args, false).await,
        ReleaseCommands::Agent(args) => agent(&args).await,
        ReleaseCommands::Proxy(args) => crate::release_agent::proxy(&args.state, &args.bind)
            .await
            .map_err(CmdError::click),
        ReleaseCommands::Status(args) => status(&args).await,
        ReleaseCommands::ActiveBinary(args) => active_binary(&args).await,
        ReleaseCommands::Logs(args) => crate::cli::release_evidence::dispatch_logs(&args).await,
        ReleaseCommands::Doctor(args) => crate::cli::release_evidence::dispatch_doctor(&args).await,
        ReleaseCommands::Quarantine(sub) => crate::cli::release_quarantine::dispatch(sub).await,
        ReleaseCommands::Rollback(args) => rollback(&args).await,
        ReleaseCommands::InstallLocal(args) => install_local(&args).await,
        ReleaseCommands::ConvergeLocalReaders(args) => converge_local_readers(&args).await,
        ReleaseCommands::ClaimCoordinate(args) => claim_coordinate(&args).await,
        ReleaseCommands::DeclareVersion(args) => {
            crate::cli::host::declare_version(
                &args.host,
                &args.binary,
                args.version.as_deref(),
                args.unset,
                args.json,
            )
            .await
        }
        ReleaseCommands::PromoteVersion(args) => {
            crate::cli::host::promote_version(&args.host, &args.binary, &args.version, args.json)
                .await
        }
        ReleaseCommands::ActivateStaged(args) => {
            crate::cli::host::activate_staged_release(
                &args.host,
                &args.product,
                &args.env_file,
                args.port,
                args.json,
            )
            .await
        }
        ReleaseCommands::VerifyPlatform(args) => {
            crate::cli::host::verify_release_platform(
                &args.host,
                &args.repo,
                &args.revision,
                args.json,
            )
            .await
        }
        ReleaseCommands::HostState(args) => {
            crate::cli::service_converge::converge(
                &args.host,
                args.binary.as_deref(),
                args.apply,
                args.json,
            )
            .await
        }
        ReleaseCommands::Provenance(args) => {
            crate::cli::host::provenance(&args.host, args.json).await
        }
    }
}
