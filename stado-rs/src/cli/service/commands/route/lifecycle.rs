//! Where the lifecycle verbs and the two unit reads land.

use super::*;

use crate::cli::service::lifecycle::adopt::adopt;
use crate::cli::service::lifecycle::adopt::handoff::control::handoff_release_control;
use crate::cli::service::lifecycle::adopt::removal::{remove, retire};
use crate::cli::service::lifecycle::adopt::{onboarding, OnboardingOptions};
use crate::cli::service::lifecycle::declare::declare;
use crate::cli::service::lifecycle::declare::ensure::run::ensure;
use crate::cli::service::lifecycle::declare::ensure::EnsureOptions;
use crate::cli::service::lifecycle::deploy::{deploy, DeployOptions};
use crate::cli::service::reports::view::{env, logs};

use super::super::spec::lifecycle::LifecycleCommands;

pub(crate) async fn dispatch(command: LifecycleCommands) -> Result<(), CmdError> {
    match command {
        LifecycleCommands::Adopt {
            unit,
            host,
            host_heuristic,
            json,
        } => adopt(&unit, host.as_deref(), host_heuristic.as_deref(), json).await,
        LifecycleCommands::Onboarding {
            name,
            host,
            product_id,
            display_name,
            repository,
            surfaces,
            first_success_fact,
            onboarding_kind,
            status,
            json,
        } => {
            onboarding(OnboardingOptions {
                name: &name,
                host: &host,
                product_id: &product_id,
                display_name: &display_name,
                repository: &repository,
                surfaces,
                first_success_fact: &first_success_fact,
                onboarding_kind: &onboarding_kind,
                status: &status,
                as_json: json,
            })
            .await
        }
        LifecycleCommands::Retire { unit, host, json } => retire(&unit, &host, json).await,
        LifecycleCommands::HandoffReleaseControl {
            service,
            host,
            product,
            json,
        } => handoff_release_control(&service, &host, &product, json).await,
        LifecycleCommands::Remove { unit, host, json } => remove(&unit, &host, json).await,
        LifecycleCommands::Deploy {
            name,
            host,
            host_heuristic,
            from,
            from_artifact,
            args,
            launchd_label,
            as_launch_agent,
            json,
        } => {
            deploy(DeployOptions {
                name: &name,
                host: host.as_deref(),
                host_heuristic: host_heuristic.as_deref(),
                from,
                from_artifact,
                args: &args,
                launchd_label: launchd_label.as_deref(),
                as_launch_agent,
                as_json: json,
            })
            .await
        }
        LifecycleCommands::Declare { file, json } => declare(&file, json).await,
        LifecycleCommands::Ensure {
            name,
            host,
            from,
            args,
            env,
            reason,
            as_daemon,
            as_launch_agent,
            json,
        } => {
            ensure(EnsureOptions {
                name: &name,
                host: &host,
                from: from.as_deref(),
                args: &args,
                env: &env,
                reason: &reason,
                as_daemon,
                as_launch_agent,
                as_json: json,
            })
            .await
        }
        LifecycleCommands::Logs {
            name,
            host,
            lines,
            json,
        } => logs(&name, host.as_deref(), lines, json).await,
        LifecycleCommands::Env { name, host, json } => env(&name, host.as_deref(), json).await,
    }
}
