//! Where the platform verbs land: the catalogs, the configuration, the hosts
//! and the services they run.
//!
//! The three arms with a nested match of their own live beside this one:
//! `registries`, `identities` and `hosts`.

use crate::cli::entry::spec::root::platform::PlatformCommands;
use crate::cli::*;

mod hosts;
mod identities;
mod registries;

pub(crate) async fn dispatch(command: PlatformCommands) -> Result<(), CmdError> {
    match command {
        PlatformCommands::Profiles { name } => profiles_cmd::run(name.as_deref()),
        PlatformCommands::Config { sub, key, value } => {
            config_cmd::run(&sub, key.as_deref(), value.as_deref())
        }
        PlatformCommands::Schedule(sub) => match sub {
            ScheduleCommands::Create(args) => schedule::create(&args).await,
            ScheduleCommands::List => schedule::list().await,
            ScheduleCommands::Show { schedule_id } => schedule::show(&schedule_id).await,
            ScheduleCommands::Rm { schedule_id } => schedule::rm(&schedule_id).await,
            ScheduleCommands::Pause { schedule_id } => schedule::pause(&schedule_id).await,
            ScheduleCommands::Resume { schedule_id } => schedule::resume(&schedule_id).await,
            ScheduleCommands::Run {
                schedule_id,
                retry_token,
            } => schedule::run(&schedule_id, &retry_token).await,
        },
        PlatformCommands::Artifact(sub) => artifact::dispatch(sub).await,
        PlatformCommands::Release(sub) => release_cmd::dispatch(sub).await,
        PlatformCommands::Cost(sub) => cost::dispatch(&sub).await,
        PlatformCommands::Vast(sub) => vast::dispatch(&sub).await,
        PlatformCommands::Quota { json, sub } => quota::dispatch(json, &sub).await,
        PlatformCommands::Registry(sub) => registries::dispatch(sub).await,
        PlatformCommands::Builds(sub) => builds::run(sub).await,
        PlatformCommands::Fleet(sub) => fleet::run(sub).await,
        PlatformCommands::Identity(sub) => identities::dispatch(sub).await,
        PlatformCommands::Host(sub) => hosts::dispatch(sub).await,
        PlatformCommands::Bootstrap {
            target,
            dry_run,
            local,
        } => bootstrap::run(target, dry_run, local).await,
        PlatformCommands::Recovery(sub) => recovery::dispatch(sub).await,
        PlatformCommands::Storage(sub) => storage::dispatch(sub).await,
        PlatformCommands::Instances(sub) => instances::dispatch(sub).await,
        PlatformCommands::Secrets(sub) => secrets::dispatch(sub).await,
        PlatformCommands::Queue(sub) => queue::dispatch(sub).await,
        PlatformCommands::Alerts(sub) => alerts::dispatch(sub).await,
        PlatformCommands::Service(sub) => service::dispatch(sub).await,
        PlatformCommands::Egress(sub) => egress::dispatch(sub).await,
        PlatformCommands::Product(sub) => product::dispatch(sub).await,
        PlatformCommands::Placement(sub) => placement::dispatch(sub).await,
        PlatformCommands::Resolver(sub) => resolver::dispatch(sub).await,
        PlatformCommands::Database(sub) => database::dispatch(sub).await,
        PlatformCommands::Web(sub) => web::dispatch(sub).await,
        PlatformCommands::Dns(sub) => dns::dispatch(sub).await,
        PlatformCommands::Inference(sub) => inference::dispatch(sub).await,
        PlatformCommands::Stream(sub) => stream::dispatch(sub).await,
        PlatformCommands::Doctor(args) => doctor::dispatch(args).await,
        PlatformCommands::Workload(command) => workload::dispatch(command).await,
        PlatformCommands::Repair(args) => repair::dispatch(args).await,
        PlatformCommands::Runner(sub) => runner::run(sub).await,
        PlatformCommands::Space(command) => space::dispatch(command).await,
        PlatformCommands::Route(sub) => route::dispatch(sub).await,
        PlatformCommands::Scratch(sub) => scratch::dispatch(sub).await,
    }
}
