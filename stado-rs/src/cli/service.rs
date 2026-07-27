//! `stado service ...` — the full service-management layer.
//!
//! NO Python original: the Python CLI stops at `host recover`, and that gap
//! is the point. `docs/missing-commands.md` items seven through fourteen
//! were written after a wedged `com.wisent.weles-api` sat unmanaged on a
//! mac mini: the unit existed on the host, Stado did not know about it, and
//! there was no command to list it, restart it or adopt it.
//!
//! The engine is [`crate::deploy::service`]; this module is the operator
//! surface over it. Two properties are worth keeping when editing:
//!
//! - `list` and `status` answer from the health beacons alone. No ssh, no
//!   per-host round trip, so the fleet-wide question stays answerable when
//!   a host is the thing that is broken.
//! - `adopt`, `retire` and `deploy` mutate the canonical registry through
//!   `cli/registry.rs::{fetch_document, push_document}` — the validated
//!   write path — and never hand-edit the document. `push_document`
//!   validates before it writes, so a mutation that would produce an
//!   invalid registry is refused with nothing uploaded.

use clap::Subcommand;
use serde_json::Value;

use crate::deploy::service::{
    self, ManagedService, ServiceEnv, ServiceLog, ServiceStatus, SOURCE_RECOVERY,
};
use crate::deploy::{host_channel, production_runner, DeployError};
use crate::queue::JobStorage;
use crate::targets;

use super::{registry, table, CmdError};

#[derive(Subcommand)]
pub enum ServiceCommands {
    /// Every registry-managed service across all hosts, with its state.
    ///
    /// Answered from the latest health beacons, so it costs no ssh and
    /// reports on hosts that are not currently reachable. A host that has
    /// published no beacon reports `unknown`, which is deliberately not
    /// the same answer as `missing`.
    List {
        #[arg(long)]
        json: bool,
    },

    /// One service's state everywhere it is managed.
    Status {
        /// Service name, or the host's own name for the unit.
        name: String,
        #[arg(long)]
        json: bool,
    },

    /// Restart one managed unit, without a full host-recovery pass.
    Restart {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to restart it everywhere it
        /// is managed.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Bring an existing launchd/systemd unit under management.
    ///
    /// The unit must already exist on the host — adoption claims what is
    /// there, it does not create anything. The host is probed first and the
    /// registry records what the host reported, not what was assumed.
    Adopt {
        /// launchd label or systemd unit name, as the host knows it.
        unit: String,
        /// Registry host that runs it.
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },

    /// Remove a service from management: bootout/disable and forget.
    ///
    /// Unit files are left on disk. Retiring is a management decision, not
    /// a deletion.
    Retire {
        /// launchd label or systemd unit name, as the host knows it.
        unit: String,
        /// Registry host that runs it.
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },

    /// Install a new unit under management: render, push, bootstrap,
    /// record.
    Deploy {
        /// Service name; lowercase letters, digits, '.', '-' and '_'.
        name: String,
        /// Registry host to install it on.
        #[arg(long)]
        host: String,
        /// Absolute path, ON THE TARGET HOST, of the program the unit runs.
        /// The plist / systemd unit is rendered around it by the same
        /// renderer `stado bootstrap --local` uses.
        #[arg(long)]
        from: String,
        #[arg(long)]
        json: bool,
    },

    /// Tail a managed unit's log over the approved channel.
    Logs {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to tail every host that
        /// manages it.
        #[arg(long)]
        host: Option<String>,
        /// Lines of tail to fetch.
        #[arg(long, default_value_t = default_log_lines())]
        lines: usize,
        #[arg(long)]
        json: bool,
    },

    /// The effective environment a managed unit runs with, secrets
    /// redacted.
    ///
    /// Parsed from the unit's own plist / systemd unit file. Values whose
    /// variable name looks like a credential are replaced, in the table and
    /// in `--json` alike.
    Env {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to read every host that
        /// manages it.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// Default `--lines` for `service logs`: one byte's worth of lines. Derived
/// from `u8::MAX` rather than written as a number, the same way
/// `cli/mod.rs::default_mail_results` derives its default from `u8::BITS`.
fn default_log_lines() -> usize {
    usize::from(u8::MAX)
}

pub async fn dispatch(command: ServiceCommands) -> Result<(), CmdError> {
    match command {
        ServiceCommands::List { json } => list(json).await,
        ServiceCommands::Status { name, json } => status(&name, json).await,
        ServiceCommands::Restart { name, host, json } => {
            restart(&name, host.as_deref(), json).await
        }
        ServiceCommands::Adopt { unit, host, json } => adopt(&unit, &host, json).await,
        ServiceCommands::Retire { unit, host, json } => retire(&unit, &host, json).await,
        ServiceCommands::Deploy {
            name,
            host,
            from,
            json,
        } => deploy(&name, &host, &from, json).await,
        ServiceCommands::Logs {
            name,
            host,
            lines,
            json,
        } => logs(&name, host.as_deref(), lines, json).await,
        ServiceCommands::Env { name, host, json } => env(&name, host.as_deref(), json).await,
    }
}

// ---------------------------------------------------------------------------
// Shared resolution
// ---------------------------------------------------------------------------

fn click(exc: DeployError) -> CmdError {
    CmdError::click(exc.to_string())
}

/// Beacons live in the registry bucket, not necessarily `WC_BUCKET` — the
/// same store `cli/host.rs::health` reads them from.
async fn beacon_store() -> Result<JobStorage, CmdError> {
    let bucket = targets::GCS_REGISTRY_URI
        .split_once("//")
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default())
        .unwrap_or_default();
    Ok(JobStorage::with_bucket(bucket).await?)
}

/// The declared managed set matching NAME, without touching beacons.
///
/// The write-side commands need the declaration — its unit id and its
/// unit-file path — not its state, so they must not pay for a beacon read
/// per host to get it.
async fn declared_matching(
    name: &str,
    host: Option<&str>,
) -> Result<Vec<ManagedService>, CmdError> {
    if let Some(host) = host {
        // Resolve the host first so an unknown or non-local target reports
        // the registry's own precise refusal rather than "no such service".
        host_channel::canonical_target(host).await.map_err(click)?;
    }
    let registry = targets::fetch_registry_remote()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let mut found: Vec<ManagedService> = Vec::new();
    for target in registry.local_targets() {
        if host.is_some_and(|host| target.name != host) {
            continue;
        }
        found.extend(
            service::declared_services(target)
                .into_iter()
                .filter(|declared| declared.matches(name)),
        );
    }
    if found.is_empty() {
        return Err(unmanaged(name, host));
    }
    Ok(found)
}

fn unmanaged(name: &str, host: Option<&str>) -> CmdError {
    match host {
        Some(host) => CmdError::click(format!(
            "{name} is not a registry-managed service on {host}"
        )),
        None => CmdError::click(format!("no registry-managed service named {name}")),
    }
}

/// `-` for an empty cell, the spelling `monitor/host_health.rs` already
/// prints for a beacon field it does not have.
fn dash(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Read commands
// ---------------------------------------------------------------------------

async fn list(json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::list_services(&store).await.map_err(click)?;
    render_status(&rows, json)
}

async fn status(name: &str, json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::find_services(&store, name).await.map_err(click)?;
    if rows.is_empty() {
        return Err(unmanaged(name, None));
    }
    render_status(&rows, json)
}

fn render_status(rows: &[ServiceStatus], json: bool) -> Result<(), CmdError> {
    if json {
        let payload: Vec<Value> = rows.iter().map(ServiceStatus::to_json).collect();
        return print_json(&Value::Array(payload));
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.service.host.clone(),
                row.service.name.clone(),
                row.service.unit_id().to_string(),
                row.service.source.clone(),
                row.state.clone(),
                dash(&row.reported_at),
                dash(&row.detail),
            ]
        })
        .collect();
    table::print(
        &[
            "HOST",
            "SERVICE",
            "UNIT",
            "SOURCE",
            "STATE",
            "REPORTED_AT",
            "DETAIL",
        ],
        &cells,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Restart
// ---------------------------------------------------------------------------

async fn restart(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let report = service::restart_service(&target, declared, &runner)
            .await
            .map_err(click)?;
        if !report.succeeded("restarted") {
            failures.push(format!("{}: {}", declared.host, report.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&report.status),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "STATUS", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "restart")
}

/// Report a partial failure after the per-host results have been printed,
/// so the operator sees which hosts worked as well as which did not.
fn fail_if_any(failures: &[String], action: &str) -> Result<(), CmdError> {
    if failures.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{action} failed on {}",
        failures.join("; ")
    )))
}

// ---------------------------------------------------------------------------
// Adopt / retire / deploy — the registry mutations
// ---------------------------------------------------------------------------

/// Declare a service through the validated write path.
///
/// `push_document` runs `targets::validate_registry` before it writes, so a
/// declaration that would produce an invalid registry is refused with
/// nothing uploaded. Returns the new generation.
async fn record_declaration(record: &ManagedService) -> Result<String, CmdError> {
    let mut document = registry::fetch_document().await?;
    service::add_service(&mut document, record).map_err(click)?;
    registry::push_document(&document).await
}

async fn adopt(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();
    let report = service::probe_service(&target, unit, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("probed") {
        return Err(CmdError::click(format!(
            "{host}: could not probe {unit}: {}",
            report.failure()
        )));
    }
    // Adoption claims a unit that is already there. Declaring one that is
    // not present is how a registry starts describing a fleet that does not
    // exist, which is the failure this command was written against.
    if report.file_state != "present" && report.unit_state != "loaded" {
        return Err(CmdError::click(format!(
            "{unit} is not present on {host}: no unit file at {} and the init system does not know it",
            report.path
        )));
    }

    let record = service::record_from_report(host, unit, &report, &now());
    let generation = record_declaration(&record).await?;
    render_mutation(
        "adopted",
        &record,
        &generation,
        Some(&report.to_json()),
        json,
    )
}

async fn retire(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let declared = service::declared_services(&target);
    let Some(found) = declared.iter().find(|candidate| candidate.matches(unit)) else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::click(format!(
            "{unit} is carried by the fixed host-recovery program, not by the registry entry \
             for {host}; it cannot be retired. Adopt it first if you need it under registry \
             management."
        )));
    }

    let runner = production_runner();
    let report = service::retire_service(&target, found, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("retired") {
        // Forgetting a unit that is still running is exactly the state this
        // command family exists to prevent, so the declaration stays until
        // the host confirms it is stopped.
        return Err(CmdError::click(format!(
            "{host}: could not stop {unit}: {}; it is still declared in the registry",
            report.failure()
        )));
    }

    let mut document = registry::fetch_document().await?;
    let removed = service::remove_service(&mut document, host, unit).map_err(click)?;
    let generation = registry::push_document(&document).await?;
    render_mutation(
        "retired",
        &removed,
        &generation,
        Some(&report.to_json()),
        json,
    )
}

async fn deploy(name: &str, host: &str, from: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let plan = service::plan_deploy(name, from).map_err(click)?;

    // Refuse a colliding declaration BEFORE touching the host: pushing a
    // unit that then cannot be recorded would leave an unmanaged unit
    // running, which is the whole failure this command family closes.
    let declared = service::declared_services(&target);
    for taken in [name, plan.label.as_str(), plan.unit.as_str()] {
        if declared.iter().any(|candidate| candidate.matches(taken)) {
            return Err(CmdError::click(format!(
                "{host} already manages {taken}; retire it first"
            )));
        }
    }

    let runner = production_runner();
    let report = service::deploy_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("deployed") {
        return Err(CmdError::click(format!(
            "{host}: could not deploy {name}: {}",
            report.failure()
        )));
    }

    let record = service::record_from_report(host, name, &report, &now());
    let generation = match record_declaration(&record).await {
        Ok(generation) => generation,
        // The unit is on the host and running; only the declaration failed.
        // Reporting that as a bare registry error would leave exactly the
        // running-but-unmanaged state this command family closes, so say
        // what happened and name the one command that repairs it.
        Err(exc) => {
            let detail = exc
                .message
                .unwrap_or_else(|| "registry write failed".to_string());
            return Err(CmdError::click(format!(
                "{host}: {name} is deployed and running, but recording it failed: {detail}. \
                 Run `stado service adopt {} --host {host}` to bring it under management.",
                record.unit_id()
            )));
        }
    };
    render_mutation(
        "deployed",
        &record,
        &generation,
        Some(&report.to_json()),
        json,
    )
}

/// `datetime.now(timezone.utc).isoformat()` as every other writer in the
/// crate stamps it (`queue/leases.rs::now_iso`).
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn render_mutation(
    action: &str,
    record: &ManagedService,
    generation: &str,
    remote: Option<&Value>,
    json: bool,
) -> Result<(), CmdError> {
    if json {
        let mut payload = serde_json::json!({
            "action": action,
            "service": record.to_json(),
            "registry_generation": generation,
        });
        if let Some(remote) = remote {
            payload["remote"] = remote.clone();
        }
        return print_json(&payload);
    }
    table::print(
        &[
            "ACTION",
            "HOST",
            "SERVICE",
            "UNIT",
            "KIND",
            "PATH",
            "GENERATION",
        ],
        &[vec![
            action.to_string(),
            record.host.clone(),
            record.name.clone(),
            record.unit_id().to_string(),
            record.kind.clone(),
            dash(&record.path),
            generation.to_string(),
        ]],
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

async fn logs(name: &str, host: Option<&str>, lines: usize, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut tails: Vec<ServiceLog> = Vec::new();
    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        tails.push(
            service::tail_logs(&target, declared, lines, &runner)
                .await
                .map_err(click)?,
        );
    }

    if json {
        let payload: Vec<Value> = tails.iter().map(ServiceLog::to_json).collect();
        return print_json(&Value::Array(payload));
    }
    for tail in &tails {
        // A log body is not tabular; it is the file. Head each one so a
        // multi-host tail stays attributable.
        println!("\n== {} {} ({}) ==", tail.host, tail.unit, tail.origin);
        print!("{}", tail.body);
        if !tail.body.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Env
// ---------------------------------------------------------------------------

async fn env(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut environments: Vec<ServiceEnv> = Vec::new();
    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let unit = service::fetch_unit_file(&target, declared, &runner)
            .await
            .map_err(click)?;
        environments.push(service::unit_environment(&unit).map_err(click)?);
    }

    if json {
        let payload: Vec<Value> = environments.iter().map(ServiceEnv::to_json).collect();
        return print_json(&Value::Array(payload));
    }

    let cells: Vec<Vec<String>> = environments
        .iter()
        .flat_map(|environment| {
            environment.env.iter().map(|(key, value)| {
                vec![
                    environment.host.clone(),
                    environment.unit.clone(),
                    key.clone(),
                    value.clone(),
                ]
            })
        })
        .collect();
    table::print(&["HOST", "UNIT", "VARIABLE", "VALUE"], &cells);

    for environment in &environments {
        for file in &environment.environment_files {
            // The pointer, not the contents: reporting it is how the
            // operator learns this listing is partial.
            println!(
                "{} {}: also reads EnvironmentFile={file} (not shown)",
                environment.host, environment.unit
            );
        }
    }
    Ok(())
}
