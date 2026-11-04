//! `stado market node add|edit|remove|list|show|follow`: a network's own
//! node run on a fleet host while the fleet's queue leaves the host idle.
//!
//! The node is a service the operator declared with `stado service declare`
//! from the network's official release; [`crate::providers::node`] keeps
//! which service earns from which source and what it holds. `follow` is the
//! loop: after `--idle-window-s` idle seconds, with the node offered, it
//! takes the node's capacity reservation on its host (refused, and said so,
//! when the host has no room) and starts the service; the moment fleet work
//! appears for the host, or the operator withdraws the node, it stops the
//! service and releases the reservation. What the node earned is the
//! network's own statement, kept with `stado market payout record --source
//! <source> --host <host>`, which `stado market earnings` adds to the day.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::cli::{CmdError, NodeCommands};
use crate::primitives::failure::FailureCode;
use crate::providers::node::{self, NodeDeclaration, NodeError, NodeReservation, NodeRun};
use crate::queue::capacity::reservations::HeldReservation;
use crate::queue::JobStorage;

/// A node failure as the command reports it, with its class.
fn cmd(error: NodeError) -> CmdError {
    match error {
        NodeError::Usage(message) => CmdError::usage(message),
        NodeError::NotFound(message) => CmdError::missing(message),
        NodeError::Refused(message) => CmdError::refused(message),
        NodeError::Storage(error) => CmdError::from(error),
    }
}

fn say(message: &str) {
    println!("[{}] {message}", chrono::Utc::now().to_rfc3339());
}

/// The source id must be one `stado market sources` declares, so a payout
/// recorded for the node's source joins the same ledger.
fn known_source(source: &str) -> Result<(), CmdError> {
    let declared = super::vast::source_ids()?;
    if declared.iter().any(|id| id == source) {
        return Ok(());
    }
    Err(CmdError::usage(format!(
        "--source {source:?} is no source `stado market sources` declares; it declares {}",
        declared.join(", ")
    )))
}

/// The registry host must declare the node's service, else nothing could
/// start it.
async fn declared_service(target: &str, service: &str) -> Result<(), CmdError> {
    let found = crate::cli::service::declared_matching(service, Some(target)).await?;
    if found.is_empty() {
        return Err(CmdError::refused(format!(
            "{target} declares no service {service}; declare the network's node from its \
             official release with `stado service declare` first"
        )));
    }
    Ok(())
}

fn print_node(
    verb: &str,
    declared: &NodeDeclaration,
    run: Option<&NodeRun>,
    json_output: bool,
) -> Result<(), CmdError> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"node": declared, "run": run}))?
        );
        return Ok(());
    }
    let held = declared.reservation;
    println!(
        "{verb} node {}: {} on {} earns from {}; holds {} cores, {} GiB, {} GiB VRAM; {}",
        declared.name,
        declared.service,
        declared.target,
        declared.source,
        held.cpu_cores,
        held.ram_gb,
        held.vram_gb,
        if declared.listed {
            "offered"
        } else {
            "withdrawn"
        }
    );
    if let Some(run) = run {
        println!("  last: {} ({})", run.state, run.changed_at);
    }
    Ok(())
}

/// Answer one `stado market node` command.
pub(crate) async fn dispatch(command: &NodeCommands) -> Result<(), CmdError> {
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    match command {
        NodeCommands::Add {
            name,
            source,
            target,
            service,
            cpu_cores,
            ram_gb,
            vram_gb,
            json,
        } => {
            if node::load(&store, name).await.map_err(cmd)?.is_some() {
                return Err(CmdError::refused(format!(
                    "node {name} is already declared; `stado market node edit {name}` changes it"
                )));
            }
            known_source(source)?;
            declared_service(target, service).await?;
            let now = chrono::Utc::now().to_rfc3339();
            let stored = node::save(
                &store,
                &NodeDeclaration {
                    name: name.clone(),
                    source: source.clone(),
                    target: target.clone(),
                    service: service.clone(),
                    reservation: NodeReservation {
                        cpu_cores: *cpu_cores,
                        ram_gb: *ram_gb,
                        vram_gb: *vram_gb,
                    },
                    // Declaring is not offering: the operator offers the node
                    // with `edit --listed true` once it is ready to run.
                    listed: false,
                    declared_at: now.clone(),
                    changed_at: now,
                },
            )
            .await
            .map_err(cmd)?;
            print_node("declared", &stored, None, *json)
        }
        NodeCommands::Edit {
            name,
            listed,
            cpu_cores,
            ram_gb,
            vram_gb,
            json,
        } => {
            let mut declared = node::require(&store, name).await.map_err(cmd)?;
            if listed.is_none() && cpu_cores.is_none() && ram_gb.is_none() && vram_gb.is_none() {
                return Err(CmdError::usage(
                    "edit changes --listed, --cpu-cores, --ram-gb or --vram-gb; none was given",
                ));
            }
            if let Some(listed) = listed {
                declared.listed = *listed;
            }
            if let Some(cpu_cores) = cpu_cores {
                declared.reservation.cpu_cores = *cpu_cores;
            }
            if let Some(ram_gb) = ram_gb {
                declared.reservation.ram_gb = *ram_gb;
            }
            if let Some(vram_gb) = vram_gb {
                declared.reservation.vram_gb = *vram_gb;
            }
            declared.changed_at = chrono::Utc::now().to_rfc3339();
            let stored = node::save(&store, &declared).await.map_err(cmd)?;
            let run = node::load_run(&store, name).await.map_err(cmd)?;
            print_node("changed", &stored, run.as_ref(), *json)
        }
        NodeCommands::Remove { name, json } => {
            node::require(&store, name).await.map_err(cmd)?;
            if node::load_run(&store, name)
                .await
                .map_err(cmd)?
                .is_some_and(|run| run.running)
            {
                return Err(CmdError::refused(format!(
                    "node {name} is running; `stado market node edit {name} --listed false` \
                     withdraws it, and its follow loop stops it and releases its reservation"
                )));
            }
            node::remove(&store, name).await.map_err(cmd)?;
            if *json {
                println!("{}", json!({"removed": name}));
            } else {
                println!("removed node {name}");
            }
            Ok(())
        }
        NodeCommands::List { json } => {
            let nodes = node::list(&store).await.map_err(cmd)?;
            let mut rows = Vec::new();
            for declared in &nodes {
                let run = node::load_run(&store, &declared.name).await.map_err(cmd)?;
                rows.push(json!({"node": declared, "run": run}));
            }
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"schema": "stado.market-nodes.v1", "nodes": rows})
                    )?
                );
                return Ok(());
            }
            if nodes.is_empty() {
                println!("no network node is declared; `stado market node add` declares one");
            }
            for row in &rows {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    row["node"]["name"].as_str().unwrap_or("?"),
                    row["node"]["source"].as_str().unwrap_or("?"),
                    row["node"]["target"].as_str().unwrap_or("?"),
                    row["node"]["service"].as_str().unwrap_or("?"),
                    if row["node"]["listed"].as_bool() == Some(true) {
                        "offered"
                    } else {
                        "withdrawn"
                    },
                    row["run"]["state"].as_str().unwrap_or("never followed"),
                );
            }
            Ok(())
        }
        NodeCommands::Show { name, json } => {
            let declared = node::require(&store, name).await.map_err(cmd)?;
            let run = node::load_run(&store, name).await.map_err(cmd)?;
            print_node("node", &declared, run.as_ref(), *json)
        }
        NodeCommands::Follow {
            name,
            idle_window_s,
            poll_interval_s,
            dry_run,
            once,
        } => {
            follow(
                &store,
                name,
                *idle_window_s,
                *poll_interval_s,
                *dry_run,
                *once,
            )
            .await
        }
    }
}

/// The workload the node's reservation is taken as: the capacity primitive
/// reserves a declared kind, and a node's kind is its declaration.
fn reserved_kind(declared: &NodeDeclaration) -> crate::cli::workload::WorkloadKind {
    crate::cli::workload::WorkloadKind {
        kind: format!("market-node-{}", declared.name),
        product: "stado".to_string(),
        interactive: false,
        registry_allowance: None,
        plan_schema: None,
        reservation: Some(crate::cli::workload::WorkloadReservation {
            cpu_cores: declared.reservation.cpu_cores,
            ram_gb: declared.reservation.ram_gb,
            vram_gb: declared.reservation.vram_gb,
        }),
        detachable: false,
        maintenance: false,
        park_after_seconds: None,
        harness: None,
        report: vec!["target".to_string()],
    }
}

async fn record(
    store: &JobStorage,
    name: &str,
    running: bool,
    state: &str,
    held: Option<&HeldReservation>,
    running_since: Option<&str>,
    dry_run: bool,
) -> Result<(), CmdError> {
    say(state);
    if dry_run {
        return Ok(());
    }
    node::save_run(
        store,
        &NodeRun {
            node: name.to_string(),
            running,
            state: state.to_string(),
            reservation_id: held
                .and_then(HeldReservation::reservation)
                .map(|reservation| reservation.reservation_id.clone()),
            running_since: running_since.map(str::to_string),
            changed_at: chrono::Utc::now().to_rfc3339(),
        },
    )
    .await
    .map_err(cmd)
}

/// Run the node while its host is idle, until the loop is stopped.
async fn follow(
    store: &JobStorage,
    name: &str,
    idle_window_s: i64,
    poll_interval_s: Option<u64>,
    dry_run: bool,
    once: bool,
) -> Result<(), CmdError> {
    if once && !dry_run {
        return Err(CmdError::usage(
            "--once is the one-poll preview and needs --dry-run: a node started by one poll \
             would run on with no loop holding its reservation",
        ));
    }
    let Some(poll_interval_s) = poll_interval_s.or(once.then_some(0)) else {
        return Err(CmdError::usage(
            "follow needs --poll-interval-s, the seconds between queue polls, unless it runs \
             --once",
        ));
    };
    if idle_window_s.is_negative() {
        return Err(CmdError::usage("--idle-window-s cannot be negative"));
    }
    let declared = node::require(store, name).await.map_err(cmd)?;
    let registry = crate::cli::registry::read_registry().await?;
    let target = registry
        .targets
        .iter()
        .find(|candidate| candidate.name == declared.target)
        .ok_or_else(|| {
            CmdError::click(format!(
                "node {name} runs on {}, which the registry does not declare",
                declared.target
            ))
            .stating(FailureCode::NotFound)
        })?
        .clone();
    // Fleet work names its host by hostname, as the Vast loop reads it.
    let hostname = target
        .hostnames
        .first()
        .cloned()
        .unwrap_or_else(|| target.name.clone());
    let _lock = if dry_run {
        say("DRY-RUN: decisions only; the service is neither started nor stopped");
        None
    } else {
        let program = std::env::current_exe().map_err(|error| {
            CmdError::declaration(format!("this program's path cannot be read: {error}"))
        })?;
        Some(crate::deploy::role_lock::take(
            &crate::config_file::home().map_err(CmdError::declaration)?,
            &node::loop_role(name),
            &program,
        )?)
    };
    let mut cadence = tokio::time::interval(Duration::from_secs(poll_interval_s.max(1)));
    let mut held: Option<HeldReservation> = None;
    // Unknown until the loop has acted: a node left running by an earlier
    // loop is stopped on the first poll that should not run it.
    let mut running: Option<bool> = None;
    let mut idle_since: Option<Instant> = None;
    loop {
        cadence.tick().await;
        let current = node::require(store, name).await.map_err(cmd)?;
        let state = crate::providers::vast::is_stado_busy(store, &hostname)
            .await
            .map_err(CmdError::from)?;
        let withdraw = if !current.listed {
            Some("withdrawn by the operator".to_string())
        } else if !state.idle {
            Some(format!(
                "the fleet has work for {} (queued={}, running_here={})",
                current.target, state.queued, state.running_here
            ))
        } else {
            None
        };
        match withdraw {
            Some(reason) => {
                idle_since = None;
                if running != Some(false) {
                    if !dry_run {
                        crate::cli::service::stop(
                            &current.service,
                            Some(&current.target),
                            None,
                            false,
                        )
                        .await?;
                        if let Some(lease) = held.take() {
                            lease.release().await.map_err(CmdError::from)?;
                        }
                    }
                    running = Some(false);
                    record(
                        store,
                        name,
                        false,
                        &format!("stopped: {reason}"),
                        None,
                        None,
                        dry_run,
                    )
                    .await?;
                } else {
                    say(&format!("not running: {reason}"));
                }
            }
            None if running == Some(true) => say("running: the host is idle"),
            None => {
                let since = *idle_since.get_or_insert_with(Instant::now);
                let idle_for = since.elapsed().as_secs();
                if idle_for < idle_window_s.unsigned_abs() {
                    say(&format!(
                        "idle {idle_for}s of the {idle_window_s}s window before the node starts"
                    ));
                } else if dry_run {
                    record(
                        store,
                        name,
                        false,
                        "would start: the host is idle",
                        None,
                        None,
                        true,
                    )
                    .await?;
                } else {
                    let holder = format!("market node {name} pid {}", std::process::id());
                    match crate::cli::capacity::reserve_for_workload(
                        &reserved_kind(&current),
                        &target,
                        holder,
                    )
                    .await?
                    {
                        Err(refusal) => {
                            record(
                                store,
                                name,
                                false,
                                &format!("not started: {}", refusal.sentence),
                                None,
                                None,
                                false,
                            )
                            .await?;
                        }
                        Ok(lease) => {
                            if let Err(error) = crate::cli::service::restart_quietly(
                                &current.service,
                                Some(&current.target),
                            )
                            .await
                            {
                                lease.release().await.map_err(CmdError::from)?;
                                record(
                                    store,
                                    name,
                                    false,
                                    &format!(
                                        "not started: {} did not start: {error}",
                                        current.service
                                    ),
                                    None,
                                    None,
                                    false,
                                )
                                .await?;
                            } else {
                                let since = chrono::Utc::now().to_rfc3339();
                                record(
                                    store,
                                    name,
                                    true,
                                    "running: the host is idle",
                                    Some(&lease),
                                    Some(&since),
                                    false,
                                )
                                .await?;
                                held = Some(lease);
                                running = Some(true);
                            }
                        }
                    }
                }
            }
        }
        if once {
            return Ok(());
        }
    }
}
