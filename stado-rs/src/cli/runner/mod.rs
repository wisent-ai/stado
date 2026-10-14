//! `stado runner` — declared GitHub runner profiles across the fleet.

mod diagnostics;
mod outcome;
mod render;

use clap::Subcommand;
use serde_json::Value;

use outcome::render_lifecycle;
use render::{render_fleet, render_profiles, render_status_set};

use super::CmdError;

#[derive(Subcommand)]
pub enum RunnerCommands {
    /// List the runner profiles compiled into this Stado build.
    List {
        /// Emit the declaration as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install or reconcile one declared profile on a registry host.
    Install {
        target: String,
        /// Profile name from stado-rs/data/work/runner-profiles.json.
        #[arg(long)]
        profile: String,
        /// Register against this repository instead of the organization.
        #[arg(long)]
        repository: Option<String>,
        /// Emit the lifecycle report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read one or every declared profile on a registry host.
    Status {
        target: String,
        /// Restrict the read to one declared profile.
        #[arg(long)]
        profile: Option<String>,
        /// Emit the typed status report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Restart one declared runner by cycling its role on the host's Stado
    /// unit, and read GitHub's view of it afterwards.
    Restart {
        target: String,
        /// Profile name from stado-rs/data/work/runner-profiles.json.
        #[arg(long)]
        profile: String,
        /// Repository scope used when this runner was registered.
        #[arg(long)]
        repository: Option<String>,
        /// Emit the lifecycle report for native clients.
        #[arg(long, hide = true)]
        json: bool,
    },
    /// Deregister and remove one declared runner from a registry host.
    Remove {
        target: String,
        /// Profile name from stado-rs/data/work/runner-profiles.json.
        #[arg(long)]
        profile: String,
        /// Repository scope used when this runner was registered.
        #[arg(long)]
        repository: Option<String>,
        /// Emit the lifecycle report for native clients.
        #[arg(long, hide = true)]
        json: bool,
    },
    /// Read installed profiles, registration scopes, listeners and job slots fleet-wide.
    Report {
        /// Emit the fleet report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resolve the declared GitHub credential route and confront it with GitHub.
    Credential {
        /// Emit the resolution and GitHub's answer as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Mint the repository's Brama model-review bearer and install it as that
    /// repository's secret. Registering a runner does not do this: a Brama that
    /// refuses the route would otherwise stop a repository from getting a
    /// runner at all, for a secret its checks may never read.
    ModelReview {
        target: String,
        /// Repository inside wisent-ai whose CI presents the bearer.
        #[arg(long)]
        repository: String,
        /// Emit the reconciliation report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read what a runner that will not start is saying: its unit's verdict and
    /// its own diagnostic log, whole.
    Diagnostics {
        target: String,
        /// Profile name from stado-rs/data/work/runner-profiles.json.
        #[arg(long)]
        profile: String,
        /// Emit the unit verdict and the log tail as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn click(error: crate::deploy::DeployError, json: bool) -> CmdError {
    CmdError::from(error).machine_readable(json)
}

pub(crate) fn print_json(value: &Value) {
    println!("{}", crate::deploy::host_recovery::to_sorted_pretty(value));
}

/// A field the host's report carries as text, or that it did not report one,
/// in the same words the listener reading uses; a dash read as a value.
pub(crate) fn text(value: Option<&Value>) -> &str {
    match value.and_then(Value::as_str) {
        Some(text) => text,
        None => "not reported by the host",
    }
}

/// The listener as the host reported it: its connected flag, else the
/// state it published, else that it reported neither — the same reading the
/// Desktop runner summary gives.
pub(crate) fn connected(value: &Value) -> &str {
    let Some(listener) = value.get("listener") else {
        return "not reported by the host";
    };
    match listener.get("connected").and_then(Value::as_bool) {
        Some(true) => "connected",
        Some(false) => "disconnected",
        None => match listener.get("state").and_then(Value::as_str) {
            Some(state) => state,
            None => "not reported by the host",
        },
    }
}

pub async fn run(command: RunnerCommands) -> Result<(), CmdError> {
    match command {
        RunnerCommands::List { json } => render_profiles(json),
        RunnerCommands::Install {
            target,
            profile,
            repository,
            json,
        } => {
            let report = crate::deploy::host_precheck_runner::install_declared(
                &target,
                &profile,
                repository.as_deref(),
            )
            .await
            .map_err(|error| click(error, json))?;
            render_lifecycle(&report, json)
        }
        RunnerCommands::Status {
            target,
            profile,
            json,
        } => match profile {
            Some(profile) => {
                let report =
                    crate::deploy::host_precheck_runner::status_declared(&target, &profile)
                        .await
                        .map_err(|error| click(error, json))?;
                render_lifecycle(&report, json)
            }
            None => {
                let report = crate::deploy::host_precheck_runner::status_all(&target)
                    .await
                    .map_err(|error| click(error, json))?;
                render_status_set(&report, json);
                Ok(())
            }
        },
        RunnerCommands::Restart {
            target,
            profile,
            repository,
            json,
        } => {
            let report = crate::deploy::host_precheck_runner::restart_declared(
                &target,
                &profile,
                repository.as_deref(),
            )
            .await
            .map_err(|error| click(error, json))?;
            render_lifecycle(&report, json)
        }
        RunnerCommands::Remove {
            target,
            profile,
            repository,
            json,
        } => {
            let report = crate::deploy::host_precheck_runner::remove_declared(
                &target,
                &profile,
                repository.as_deref(),
            )
            .await
            .map_err(|error| click(error, json))?;
            render_lifecycle(&report, json)
        }
        RunnerCommands::Report { json } => {
            let report = crate::deploy::host_precheck_runner::fleet_report()
                .await
                .map_err(|error| click(error, json))?;
            render_fleet(&report, json);
            Ok(())
        }
        RunnerCommands::Credential { json } => crate::github_identity::report(json).await,
        RunnerCommands::ModelReview {
            target,
            repository,
            json,
        } => {
            let report = crate::deploy::host_precheck_runner::reconcile_model_review_secret(
                &target,
                &repository,
            )
            .await
            .map_err(|error| click(error, json))?;
            if json {
                print_json(&report);
            } else {
                println!(
                    "{} carries {} for {}, minted as {} on the {} route",
                    text(report.get("repository")),
                    text(report.get("secret")),
                    text(report.get("model")),
                    text(report.get("client_id")),
                    text(report.get("primary_route")),
                );
            }
            Ok(())
        }
        RunnerCommands::Diagnostics {
            target,
            profile,
            json,
        } => diagnostics::render(&target, &profile, json).await,
    }
}
