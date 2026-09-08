//! The read-only operation history subcommands: one line per archived
//! operation, or one operation's plan, state and every event it recorded. An
//! operation whose state cannot be read is reported, never skipped.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::resources::journal::names::validate_operation_id;
use crate::cli::resources::OperationsCommands;
use crate::cli::CmdError;

use super::Journal;

pub async fn dispatch(command: OperationsCommands) -> Result<(), CmdError> {
    let journal = Journal::open().await?;
    match command {
        OperationsCommands::List => list(&journal).await,
        OperationsCommands::Show { operation_id } => show(&journal, &operation_id).await,
    }
}

async fn list(journal: &Journal) -> Result<(), CmdError> {
    let names = journal
        .store
        .list_paths("operations/", usize::default())
        .await?;
    let mut ids = BTreeSet::new();
    for name in names {
        if let Some(rest) = name.strip_prefix("operations/") {
            if let Some((operation_id, _)) = rest.split_once('/') {
                if validate_operation_id(operation_id).is_ok() {
                    ids.insert(operation_id.to_string());
                }
            }
        }
    }
    let mut rows = Vec::new();
    for operation_id in ids.into_iter().rev() {
        match journal.load_state(&operation_id).await {
            Ok(state) => rows.push(json!({
                "operation_id": operation_id,
                "phase": state.phase,
                "plan_hash": state.plan_hash,
                "updated_at": state.updated_at,
                "error": state.error,
            })),
            Err(error) => rows.push(json!({
                "operation_id": operation_id,
                "phase": "unreadable",
                "error": error.to_string(),
            })),
        }
    }
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

async fn show(journal: &Journal, operation_id: &str) -> Result<(), CmdError> {
    let plan = journal.load_plan(operation_id).await?;
    let state = journal.load_state(operation_id).await?;
    let event_names = journal
        .store
        .list_paths(
            &format!("operations/{operation_id}/events/"),
            usize::default(),
        )
        .await?;
    let mut events = Vec::new();
    for path in event_names {
        if let Some(body) = journal.store.download_text(&path).await? {
            events.push(serde_json::from_str::<Value>(&body)?);
        }
    }
    events.sort_by(|left, right| {
        left["recorded_at"]
            .as_str()
            .cmp(&right["recorded_at"].as_str())
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "operation_id": operation_id,
            "plan": plan,
            "state": state,
            "events": events,
        }))?
    );
    Ok(())
}
