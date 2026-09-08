//! `identity list`: the declaration alone, with no host reached.

use anyhow::Result;
use serde_json::Value;

use super::binding_row;
use crate::cli::CmdError;
use crate::targets::load_registry_auto;

pub async fn list(json_output: bool) -> Result<(), CmdError> {
    let registry = load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let rows: Vec<Value> = registry
        .targets
        .iter()
        .flat_map(|target| {
            target
                .identities
                .iter()
                // `list` prints the declaration alone and reaches no host, so both
                // measured columns are absent here rather than guessed.
                .map(move |binding| binding_row(target, binding, None, None))
        })
        .collect();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("no host declares an identity binding");
        return Ok(());
    }
    println!("{:<24} {:<16} {:<32} USER", "HOST", "KIND", "IDENTITY");
    for row in &rows {
        println!(
            "{:<24} {:<16} {:<32} {}",
            row["host"].as_str().unwrap_or("-"),
            row["kind"].as_str().unwrap_or("-"),
            row["identity"].as_str().unwrap_or("-"),
            row["user"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}
