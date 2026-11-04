//! `stado registry history [--path P] [--json]` — who changed the registry,
//! or one part of it, when, from which host and session, with which command.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::targets::RegistryStore;

const HISTORY_SCHEMA: &str = "stado.registry-history.v1";

/// Print the recorded writes that changed `path` (every write when `None`),
/// oldest first. Writes made before records were kept, or whose record could
/// not be stored, are not invented: a record that does not follow the one
/// before it says so, and an empty history says that no write was recorded.
pub async fn history(path: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let records = store.write_history(path).await.map_err(|error| {
        CmdError::from(error).within(format!(
            "the registry's write records beside {} cannot be listed",
            store.location()
        ))
    })?;
    if json_output {
        let receipt = json!({
            "schema": HISTORY_SCHEMA,
            "location": store.location(),
            "path": path,
            "writes": records,
        });
        println!("{}", serde_json::to_string_pretty(&receipt)?);
        return Ok(());
    }
    if records.is_empty() {
        println!(
            "no recorded write changed {} at {}; writes made before Stado recorded who made \
             them are not listed",
            path.unwrap_or("the registry"),
            store.location()
        );
        return Ok(());
    }
    for record in &records {
        println!("{}", line(record));
    }
    Ok(())
}

/// One write as a line: when, generation, who, where, session, what changed,
/// and the command.
fn line(record: &Value) -> String {
    if let Some(cause) = record["unreadable"].as_str() {
        return format!(
            "{}: record unreadable: {cause}",
            record["key"].as_str().unwrap_or("?")
        );
    }
    let by = &record["by"];
    let text = |value: &Value, missing: &str| -> String {
        value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| missing.to_string())
    };
    let actor = match by["actor"].as_str() {
        Some(actor) => actor.to_string(),
        None => format!("unread ({})", text(&by["actor_unread"], "not recorded")),
    };
    let session = match (&by["session"], by["session_unread"].as_str()) {
        (Value::String(session), _) => format!("session {session}"),
        (_, Some(cause)) => format!("session unread ({cause})"),
        _ => "no agent session".to_string(),
    };
    let argv = by["argv"]
        .as_array()
        .map(|words| {
            words
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "command not recorded".to_string());
    let changed = match record["changed"].as_array() {
        Some(paths) if paths.iter().any(|path| path.as_str() == Some("")) => {
            "the whole document".to_string()
        }
        Some(paths) => paths
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        None => format!(
            "changed paths unknown ({})",
            text(&record["changed_unread"], "not recorded")
        ),
    };
    let applied = match by["applied_by"]["host"].as_str() {
        Some(host) => format!(", applied on {host}"),
        None => String::new(),
    };
    let relay = match by["relayed_unreadable"].as_str() {
        Some(cause) => format!(" (caller unknown: {cause})"),
        None => String::new(),
    };
    let gap = match record["unrecorded_before"].as_bool() {
        Some(true) => " [after an unrecorded write]",
        _ => "",
    };
    format!(
        "{} {}{gap}: {actor} on {}, {session}{applied}{relay}; changed {changed}; by `{argv}`",
        text(&record["recorded_at"], "?"),
        text(&record["generation"], "?"),
        text(&by["host"], "unknown host"),
    )
}
