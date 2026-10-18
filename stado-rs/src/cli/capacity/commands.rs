//! The `stado capacity` verbs: the fleet table and the reservations list.

use chrono::Utc;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::cli::registry::read_registry;
use crate::cli::reporting::{field as cell, table};
use crate::cli::CmdError;
use crate::primitives::constants;
use crate::queue::capacity::{consumer_names_target, read_publications, reservations, Publication};
use crate::targets::Registry;

#[derive(Subcommand)]
pub enum CapacityCommands {
    /// Every host's published capacity, net of the reservations held on it.
    List {
        #[arg(long)]
        json: bool,
    },
    /// The reservations held on one host, or on every host.
    Reservations {
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Take one declared workload kind's reservation on a host and hold it,
    /// heartbeating it, until this process receives SIGINT, SIGTERM or
    /// SIGHUP; the host publishes itself net of it meanwhile. Ends with the
    /// reservation released.
    Hold {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: CapacityCommands) -> Result<(), CmdError> {
    match command {
        CapacityCommands::List { json } => list(json).await,
        CapacityCommands::Reservations { target, json } => held(target.as_deref(), json).await,
        CapacityCommands::Hold { kind, target, json } => {
            super::hold::hold(&kind, &target, json).await
        }
    }
}

/// The registry target a publication key names; none when no target does.
/// An identity several targets declare is the refusal.
fn host_target(registry: &Registry, consumer: &str) -> Result<Option<String>, CmdError> {
    for candidate in &registry.targets {
        if consumer_names_target(registry, candidate, consumer).map_err(CmdError::from)? {
            return Ok(Some(candidate.name.clone()));
        }
    }
    Ok(None)
}

/// One host's row as its publication carries it: a field the publication
/// lacks (a publication from before workloads or reservations were
/// published) is null in JSON and `not reported` in the table, never a zero
/// or an empty list the host did not state.
fn host_row(target: Option<String>, consumer: &str, publication: &Publication) -> Value {
    let now = Utc::now();
    let payload = &publication.payload;
    json!({
        "target": target,
        "consumer_id": consumer,
        "published_at": payload.get("published_at"),
        "age_seconds": publication.age_seconds(now),
        "stale": publication.stale(now),
        "accepting_jobs": payload.get("accepting_jobs"),
        "admission_reason": payload.get("diag").and_then(|diag| diag.get("admission_reason")),
        "running_jobs": payload.get("running_jobs"),
        "running_workloads": payload.get("running_workloads"),
        "cpu": {"available": payload.get("available_cpu_cores"), "total": payload.get("total_cpu_cores")},
        "ram_gb": {"free": payload.get("free_ram_gb"), "total": payload.get("total_ram_gb")},
        "vram_gb": {"free": payload.get("free_vram_gb"), "total": payload.get("total_vram_gb")},
        "reserved": payload.get("reserved"),
        "reservations": payload.get("reservations"),
    })
}

fn pair(value: &Value, left: &str, right: &str) -> String {
    format!("{}/{}", cell(&value[left]), cell(&value[right]))
}

async fn list(json_output: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(CmdError::from)?;
    let publications = read_publications(&store).await.map_err(CmdError::from)?;
    let hosts = publications
        .iter()
        .map(|(consumer, publication)| {
            Ok(host_row(
                host_target(&registry, consumer)?,
                consumer,
                publication,
            ))
        })
        .collect::<Result<Vec<Value>, CmdError>>()?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema_version": constants::RESERVATION_SCHEMA_VERSION,
                "generated_at": Utc::now().to_rfc3339(),
                "hosts": hosts,
            }))?
        );
        return Ok(());
    }
    table::print(
        &[
            "TARGET",
            "ACCEPTING",
            "JOBS",
            "WORKLOADS",
            "CPU",
            "RAM GiB",
            "VRAM GiB",
            "RESERVED",
            "REASON",
            "AGE",
        ],
        &hosts
            .iter()
            .map(|host| {
                vec![
                    cell(&host["target"]),
                    cell(&host["accepting_jobs"]),
                    cell(&host["running_jobs"]),
                    cell(&host["running_workloads"]),
                    pair(&host["cpu"], "available", "total"),
                    pair(&host["ram_gb"], "free", "total"),
                    pair(&host["vram_gb"], "free", "total"),
                    format!(
                        "{}c {}g",
                        cell(&host["reserved"]["cpu_cores"]),
                        cell(&host["reserved"]["ram_gb"])
                    ),
                    cell(&host["admission_reason"]),
                    format!(
                        "{}s{}",
                        cell(&host["age_seconds"]),
                        if host["stale"] == true { " stale" } else { "" }
                    ),
                ]
            })
            .collect::<Vec<_>>(),
    );
    Ok(())
}

async fn held(target: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(CmdError::from)?;
    let now = Utc::now();
    let selected = match target {
        Some(name) => Some(
            registry
                .targets
                .iter()
                .find(|candidate| candidate.name == name)
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "target '{name}' is not declared; add it to the canonical registry"
                    ))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
                })?,
        ),
        None => None,
    };
    let all = reservations::read_all(&store)
        .await
        .map_err(CmdError::from)?;
    let mut rows: Vec<Value> = Vec::new();
    for (consumer, list) in &all {
        if let Some(target) = selected {
            if !consumer_names_target(&registry, target, consumer).map_err(CmdError::from)? {
                continue;
            }
        }
        for reservation in list {
            let mut row = serde_json::to_value(reservation)?;
            row["live"] = Value::from(reservation.is_live(now));
            row["expires_at"] = match reservation.expires_at() {
                Some(stamp) => Value::from(stamp.to_rfc3339()),
                None => Value::Null,
            };
            rows.push(row);
        }
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema_version": constants::RESERVATION_SCHEMA_VERSION,
                "generated_at": now.to_rfc3339(),
                "target": target,
                "reservations": rows,
            }))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "{}",
            match target {
                Some(name) => format!("{name} holds no reservations"),
                None => "no host holds a reservation".to_string(),
            }
        );
        return Ok(());
    }
    table::print(
        &[
            "TARGET", "KIND", "PRODUCT", "HOLDER", "CPU", "RAM GiB", "VRAM GiB", "ACQUIRED", "LIVE",
        ],
        &rows
            .iter()
            .map(|row| {
                vec![
                    cell(&row["target"]),
                    cell(&row["kind"]),
                    cell(&row["product"]),
                    cell(&row["holder"]),
                    cell(&row["cpu_cores"]),
                    cell(&row["ram_gb"]),
                    cell(&row["vram_gb"]),
                    cell(&row["acquired_at"]),
                    cell(&row["live"]),
                ]
            })
            .collect::<Vec<_>>(),
    );
    Ok(())
}
