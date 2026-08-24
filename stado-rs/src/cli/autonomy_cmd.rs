//! Operator interface for autonomy state, policy, decisions, and FinOps reports.

use std::path::PathBuf;

use clap::Subcommand;
use serde::Serialize;
use serde_json::{json, Value};

use crate::queue::JobStorage;

use super::CmdError;

#[derive(Subcommand, Debug)]
pub enum OptimizeCommands {
    /// Show mode, safety state, inventory freshness, and latest decisions.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Explain one immutable placement or resource decision.
    Explain { decision_id: String },
    /// Run one inventory, optimizer, reconciler, and FinOps cycle.
    Run,
    /// Emergency-stop new autonomous mutations.
    Pause { reason: String },
    /// Clear the emergency stop.
    Resume,
    /// Inspect or atomically replace the versioned autonomy policy.
    #[command(subcommand)]
    Policy(PolicyCommands),
}

#[derive(Subcommand, Debug)]
pub enum PolicyCommands {
    Show,
    Apply {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        expect_version: Option<String>,
    },
}

#[derive(Serialize)]
struct OptimizeStatus {
    policy: crate::autonomy::AutonomyPolicy,
    policy_version: Option<String>,
    control: crate::autonomy::storage::ControlState,
    inventory: Option<InventoryStatus>,
    decisions: usize,
    active_leases: usize,
    forecast: Option<Value>,
    anomalies: Option<Value>,
    savings: Option<Value>,
    service_reconciliation: Option<Value>,
}

#[derive(Serialize)]
struct InventoryStatus {
    snapshot_id: String,
    created_at: String,
    complete: bool,
    resources: usize,
    sources: Vec<Value>,
}

pub async fn dispatch_optimize(command: OptimizeCommands) -> Result<(), CmdError> {
    match command {
        OptimizeCommands::Status { json } => status(json).await,
        OptimizeCommands::Explain { decision_id } => explain(&decision_id).await,
        OptimizeCommands::Run => run_once().await,
        OptimizeCommands::Pause { reason } => pause(reason).await,
        OptimizeCommands::Resume => resume().await,
        OptimizeCommands::Policy(command) => policy(command).await,
    }
}

async fn status(json_output: bool) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let policy = crate::autonomy::storage::load_policy(&store).await?;
    let versioned = crate::autonomy::storage::load_policy_versioned(&store).await?;
    let control = crate::autonomy::storage::load_control(&store).await?;
    let inventory = crate::autonomy::storage::load_latest_inventory(&store)
        .await?
        .map(|snapshot| InventoryStatus {
            snapshot_id: snapshot.snapshot_id,
            created_at: snapshot.created_at,
            complete: snapshot.complete,
            resources: snapshot.resources.len(),
            sources: snapshot
                .sources
                .into_iter()
                .map(|source| {
                    json!({
                        "provider": source.provider,
                        "account": source.account,
                        "state": source.state,
                        "coverage": source.coverage,
                        "error": source.upstream_error,
                    })
                })
                .collect(),
        });
    let decisions = crate::autonomy::storage::list_decisions(&store).await?;
    let now = chrono::Utc::now();
    let active_leases = decisions
        .iter()
        .filter(|decision| {
            decision.state == "leased"
                && chrono::DateTime::parse_from_rfc3339(&decision.expires_at)
                    .is_ok_and(|expires| expires.with_timezone(&chrono::Utc) > now)
        })
        .count();
    let status = OptimizeStatus {
        policy,
        policy_version: versioned.map(|value| value.version),
        control,
        inventory,
        decisions: decisions.len(),
        active_leases,
        forecast: report_value(&store, "forecast").await?,
        anomalies: report_value(&store, "anomalies").await?,
        savings: report_value(&store, "savings").await?,
        service_reconciliation: crate::autonomy::storage::read_json(
            &store,
            "autonomy/services/latest.json",
        )
        .await?,
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    println!("Autonomy mode: {:?}", status.policy.mode);
    println!(
        "Emergency pause: {}{}",
        status.control.emergency_paused,
        status
            .control
            .reason
            .as_deref()
            .map(|reason| format!(" ({reason})"))
            .unwrap_or_default()
    );
    println!(
        "Circuit breaker: open={}, consecutive failures={}, until={}",
        status.control.circuit_open_at(chrono::Utc::now()),
        status.control.consecutive_mutation_failures,
        status
            .control
            .circuit_open_until
            .as_deref()
            .unwrap_or("not open")
    );
    println!(
        "Policy version: {}",
        status.policy_version.as_deref().unwrap_or("not persisted")
    );
    if let Some(inventory) = status.inventory {
        println!(
            "Inventory: {} resources, complete={}, snapshot={}",
            inventory.resources, inventory.complete, inventory.snapshot_id
        );
    } else {
        println!("Inventory: absent");
    }
    println!(
        "Decisions: {} (leased={})",
        status.decisions, status.active_leases
    );
    if let Some(forecast) = status.forecast {
        println!("Forecast: {}", serde_json::to_string_pretty(&forecast)?);
    }
    if let Some(anomalies) = status.anomalies {
        println!("Anomalies: {}", serde_json::to_string_pretty(&anomalies)?);
    }
    if let Some(savings) = status.savings {
        println!("Savings: {}", serde_json::to_string_pretty(&savings)?);
    }
    if let Some(services) = status.service_reconciliation {
        println!("Services: {}", serde_json::to_string_pretty(&services)?);
    }
    Ok(())
}

async fn explain(decision_id: &str) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let decision = crate::autonomy::storage::load_decision(&store, decision_id)
        .await?
        .ok_or_else(|| CmdError::click(format!("decision not found: {decision_id}")))?;
    println!("{}", serde_json::to_string_pretty(&decision)?);
    Ok(())
}

async fn run_once() -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let policy = crate::autonomy::storage::load_policy(&store).await?;
    let providers = crate::coordinator::resolve_providers();
    crate::coordinator::run_autonomy_once(&store, &providers, policy, &|message| {
        eprintln!("[autonomy] {message}");
    })
    .await?;
    println!("Autonomy cycle completed");
    Ok(())
}

async fn pause(reason: String) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let state = crate::autonomy::storage::set_control(&store, true, Some(reason), actor()).await?;
    println!("Autonomy paused at {}", state.changed_at);
    Ok(())
}

async fn resume() -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let state = crate::autonomy::storage::set_control(&store, false, None, actor()).await?;
    println!("Autonomy resumed at {}", state.changed_at);
    Ok(())
}

async fn policy(command: PolicyCommands) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    match command {
        PolicyCommands::Show => {
            let policy = crate::autonomy::storage::load_policy(&store).await?;
            let version = crate::autonomy::storage::load_policy_versioned(&store)
                .await?
                .map(|value| value.version);
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "version": version,
                    "policy": policy,
                }))?
            );
        }
        PolicyCommands::Apply {
            file,
            expect_version,
        } => {
            let raw = std::fs::read_to_string(&file).map_err(|error| {
                CmdError::click(format!("cannot read policy {}: {error}", file.display()))
            })?;
            let policy: crate::autonomy::AutonomyPolicy = serde_json::from_str(&raw)?;
            let version =
                crate::autonomy::storage::write_policy(&store, &policy, expect_version.as_deref())
                    .await?;
            println!("Policy applied atomically; version={version}");
        }
    }
    Ok(())
}

pub(crate) async fn show_report(name: &str, json_output: bool) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let value = report_value(&store, name).await?.ok_or_else(|| {
        CmdError::click(format!(
            "cost report absent: {name}; run `stado optimize run`"
        ))
    })?;
    if json_output {
        println!("{}", serde_json::to_string(&value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
    }
    Ok(())
}

async fn report_value(store: &JobStorage, name: &str) -> Result<Option<Value>, CmdError> {
    Ok(crate::autonomy::storage::read_json(store, &format!("autonomy/cost/{name}.json")).await?)
}

/// Who a recorded mutation is attributed to.
///
/// Shared with `cli/service.rs`, which stamps the audit record of a
/// `service ensure` pass: one spelling of "who did this" across everything in
/// this binary that writes an operator's name into durable state.
pub(crate) fn actor() -> String {
    std::env::var("USER").unwrap_or_else(|_| "operator".to_string())
}
