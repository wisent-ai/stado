//! `stado runner` — declared GitHub runner profiles across the fleet.

use clap::Subcommand;
use serde_json::Value;

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
        /// Profile name from stado-rs/data/runner-profiles.json.
        #[arg(long)]
        profile: String,
        /// Register against this repository and reconcile its profile secrets.
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
    /// Restart one declared runner in place and wait for a fresh listener event.
    Restart {
        target: String,
        /// Profile name from stado-rs/data/runner-profiles.json.
        #[arg(long)]
        profile: String,
        /// Emit the lifecycle report for native clients.
        #[arg(long, hide = true)]
        json: bool,
    },
    /// Deregister and remove one declared runner from a registry host.
    Remove {
        target: String,
        /// Profile name from stado-rs/data/runner-profiles.json.
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
}

fn click(error: crate::deploy::DeployError, json: bool) -> CmdError {
    CmdError::click(error.to_string()).machine_readable(json)
}

fn print_json(value: &Value) {
    println!("{}", crate::deploy::host_recovery::to_sorted_pretty(value));
}

fn text(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("-")
}

fn connected(value: &Value) -> &str {
    match value
        .get("listener")
        .and_then(|listener| listener.get("connected"))
        .and_then(Value::as_bool)
    {
        Some(true) => "connected",
        Some(false) => "disconnected",
        None => "unknown",
    }
}

fn render_lifecycle(report: &Value, json: bool) -> Result<(), CmdError> {
    if json {
        print_json(report);
    } else {
        println!(
            "{}: {} runner {}; scope {}; listener {}; host job slot {}",
            text(report.get("target")),
            text(report.get("profile")),
            text(report.get("action")),
            text(report.get("runner_scope")),
            connected(report),
            text(report.get("host_job_slot")),
        );
    }
    lifecycle_outcome(report).map_err(|error| error.machine_readable(json))
}

fn lifecycle_outcome(report: &Value) -> Result<(), CmdError> {
    if report.get("exit_code").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return Err(CmdError::click(
            report
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("the runner status command failed"),
        ));
    }
    route_outcome(report)
}

fn route_outcome(report: &Value) -> Result<(), CmdError> {
    let Some(route) = report.get("brama_route") else {
        return Ok(());
    };
    if route.get("matches").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{}: {}",
        text(report.get("target")),
        route
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("the runner's Brama route does not match the fleet declaration")
    )))
}

fn render_profiles(json: bool) -> Result<(), CmdError> {
    let declaration = crate::deploy::host_precheck_runner::runner_declaration()
        .map_err(|error| click(error, json))?;
    if json {
        print_json(&serde_json::to_value(declaration)?);
        return Ok(());
    }
    println!("PROFILE\tSLUG\tRUNNER GROUP\tLABELS");
    for profile in &declaration.profiles {
        println!(
            "{}\t{}\t{}\t{}",
            profile.name,
            profile.slug,
            profile.github_runner_group,
            profile.labels.join(",")
        );
    }
    Ok(())
}

fn render_status_set(report: &Value, json: bool) {
    if json {
        print_json(report);
        return;
    }
    for profile in report
        .get("profiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
            "{}: installed {}; scope {}; listener {}; host job slot {}",
            text(profile.get("profile")),
            profile
                .get("installed")
                .and_then(Value::as_bool)
                .map_or("unknown", |installed| if installed { "yes" } else { "no" }),
            text(profile.get("runner_scope")),
            connected(profile),
            text(profile.get("host_job_slot")),
        );
    }
}

fn render_fleet(report: &Value, json: bool) {
    if json {
        print_json(report);
        return;
    }
    println!("TARGET\tPROFILE\tINSTALLED\tSCOPE\tLISTENER\tHOST JOB SLOT");
    for host in report
        .get("hosts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for profile in host
            .get("profiles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                text(host.get("target")),
                text(profile.get("profile")),
                profile
                    .get("installed")
                    .and_then(Value::as_bool)
                    .map_or("unknown", |installed| if installed { "yes" } else { "no" }),
                text(profile.get("runner_scope")),
                connected(profile),
                text(profile.get("host_job_slot")),
            );
        }
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
            json,
        } => {
            let report = crate::deploy::host_precheck_runner::restart_declared(&target, &profile)
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
    }
}
