//! The `stado capacity` verbs: the fleet table and the reservations list.

use chrono::Utc;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::cli::registry::read_registry;
use crate::cli::reporting::table;
use crate::cli::CmdError;
use crate::primitives::constants;
use crate::queue::capacity::{
    consumer_names_target, read_publications, reservations, Publication, Reserved,
};
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
    /// Take one declared workload kind's reservation on a host and hold it
    /// for a fixed time, heartbeating it; the host publishes itself net of
    /// it meanwhile. Ends with the reservation released.
    Hold {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        seconds: u64,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: CapacityCommands) -> Result<(), CmdError> {
    match command {
        CapacityCommands::List { json } => list(json).await,
        CapacityCommands::Reservations { target, json } => held(target.as_deref(), json).await,
        CapacityCommands::Hold {
            kind,
            target,
            seconds,
            json,
        } => super::hold::hold(&kind, &target, seconds, json).await,
    }
}

fn host_row(registry: &Registry, consumer: &str, publication: &Publication) -> Value {
    let now = Utc::now();
    let payload = &publication.payload;
    let target = registry
        .targets
        .iter()
        .find(|target| consumer_names_target(registry, target, consumer))
        .map(|target| target.name.clone());
    // A publication from before reservations existed held nothing.
    let nothing_held = serde_json::to_value(Reserved::default()).unwrap_or(Value::Null);
    json!({
        "target": target,
        "consumer_id": consumer,
        "published_at": payload.get("published_at"),
        "age_seconds": publication.age_seconds(now),
        "stale": publication.stale(now),
        "accepting_jobs": payload.get("accepting_jobs"),
        "admission_reason": payload.get("diag").and_then(|diag| diag.get("admission_reason")),
        "running_jobs": payload.get("running_jobs"),
        "running_workloads": payload.get("running_workloads").cloned().unwrap_or(Value::from(0)),
        "cpu": {"available": payload.get("available_cpu_cores"), "total": payload.get("total_cpu_cores")},
        "ram_gb": {"free": payload.get("free_ram_gb"), "total": payload.get("total_ram_gb")},
        "vram_gb": {"free": payload.get("free_vram_gb"), "total": payload.get("total_vram_gb")},
        "reserved": payload.get("reserved").cloned().unwrap_or(nothing_held),
        "reservations": payload.get("reservations").cloned().unwrap_or_else(|| json!([])),
    })
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => "?".to_string(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number
            .as_f64()
            .map(|f| {
                if f.fract() == 0.0 {
                    format!("{f:.0}")
                } else {
                    format!("{f:.1}")
                }
            })
            .unwrap_or_else(|| number.to_string()),
        other => other.to_string(),
    }
}

fn pair(value: &Value, left: &str, right: &str) -> String {
    format!("{}/{}", cell(&value[left]), cell(&value[right]))
}

async fn list(json_output: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let publications = read_publications(&store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let hosts: Vec<Value> = publications
        .iter()
        .map(|(consumer, publication)| host_row(&registry, consumer, publication))
        .collect();
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
        .map_err(|error| CmdError::click(error.to_string()))?;
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
                })?,
        ),
        None => None,
    };
    let all = reservations::read_all(&store)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let rows: Vec<Value> = all
        .iter()
        .filter(|(consumer, _)| {
            selected.is_none_or(|target| consumer_names_target(&registry, target, consumer))
        })
        .flat_map(|(_, list)| list.iter())
        .map(|reservation| {
            let mut row = serde_json::to_value(reservation).unwrap_or(Value::Null);
            row["live"] = Value::from(reservation.is_live(now));
            row["expires_at"] = reservation
                .expires_at()
                .map(|stamp| Value::from(stamp.to_rfc3339()))
                .unwrap_or(Value::Null);
            row
        })
        .collect();
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
