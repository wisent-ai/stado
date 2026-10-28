//! `stado placement transactions`: the placement moves the registry holds
//! open. While one is open every service of its profile is refused to every
//! consumer, so this is where an operator reads which move holds a service,
//! between which hosts, and for how long; a move whose process ended without
//! committing or rolling back reads as one open far longer than any move runs.
//! A move this host runs or ran also shows the last step it began and whether
//! its process still runs, read from the progress it keeps here, so a move at
//! work is told from one to release without guessing from its age.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::transfer::{progress_summary, sweep_progress};
use crate::cli::{registry, CmdError};
use crate::placement::{self, PlacementTransaction};

/// One open transaction with how long it has been open, measured against this
/// host's clock at the moment of the read.
fn row(transaction: &PlacementTransaction, now: DateTime<Utc>) -> Result<Value, CmdError> {
    let started = DateTime::parse_from_rfc3339(&transaction.started_at).map_err(|error| {
        CmdError::declaration(format!(
            "placement transaction {} records started_at {:?}, which is not an RFC 3339 time: {error}",
            transaction.id, transaction.started_at
        ))
    })?;
    // The progress this host kept of the move, or why it cannot be read;
    // `recorded_here: false` when this host keeps no record (another host
    // runs the move, or it ran before records were kept).
    let progress = match progress_summary(&transaction.id) {
        Ok(Some(summary)) => summary,
        Ok(None) => json!({ "recorded_here": false }),
        Err(cause) => json!({ "unreadable": cause }),
    };
    Ok(json!({
        "id": transaction.id,
        "profile": transaction.profile,
        "from_host": transaction.from_host,
        "to_host": transaction.to_host,
        "started_at": transaction.started_at,
        "open_seconds": now.signed_duration_since(started).num_seconds(),
        "this_host_progress": progress,
    }))
}

pub(crate) async fn list(json_output: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let open = placement::transactions(&document).map_err(CmdError::declaration)?;
    let now = Utc::now();
    let rows = open
        .iter()
        .map(|transaction| row(transaction, now))
        .collect::<Result<Vec<_>, _>>()?;
    // What this host keeps of moves the registry no longer holds open is
    // spent: removed here, where every open move is known, and said on stderr
    // so standard output keeps only the answer.
    let open_ids: Vec<String> = open
        .iter()
        .map(|transaction| transaction.id.clone())
        .collect();
    match sweep_progress(&open_ids) {
        Ok(removed) => {
            for what in removed {
                eprintln!("stado: removed {what}, an ended placement move");
            }
        }
        Err(cause) => eprintln!("stado: {cause}"),
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if open.is_empty() {
        println!("no placement transaction is open; every placement profile resolves and may move");
        return Ok(());
    }
    for (transaction, row) in open.iter().zip(&rows) {
        println!(
            "{} holds placement profile {} moving it from {} to {} since {} ({} s open); its services are refused to every consumer until the move commits or rolls back",
            transaction.id,
            transaction.profile,
            transaction.from_host,
            transaction.to_host,
            transaction.started_at,
            crate::cli::reporting::field(&row["open_seconds"])
        );
        let progress = &row["this_host_progress"];
        if let Some(cause) = progress["unreadable"].as_str() {
            println!("  the progress this host kept of it cannot be read: {cause}");
        } else if let Some(step) = progress["last_step"].as_str() {
            let process = if progress["process_running"].as_bool() == Some(true) {
                "still runs, so the move is at work"
            } else {
                "has ended without committing or rolling back, so stado placement release ends the move"
            };
            println!(
                "  on this host it last began: {step} (kept {}); its process {} {process}",
                crate::cli::reporting::field(&progress["kept_at"]),
                crate::cli::reporting::field(&progress["pid"])
            );
        }
    }
    Ok(())
}
