//! `identity list`: the declaration alone, with no host reached.

use anyhow::Result;
use serde_json::Value;

use super::{binding_row, Drivability};
use crate::cli::CmdError;
use crate::targets::load_registry_auto;

pub async fn list(json_output: bool) -> Result<(), CmdError> {
    let registry = load_registry_auto().await.map_err(CmdError::from)?;
    // `list` prints the declaration alone and reaches no host, so both measured
    // columns are absent here rather than guessed.
    let unasked = Drivability {
        drivable: None,
        reason: None,
    };
    let bindings: Vec<_> = registry
        .targets
        .iter()
        .flat_map(|target| {
            target
                .identities
                .iter()
                .map(move |binding| (target, binding))
        })
        .collect();
    if json_output {
        let rows: Vec<Value> = bindings
            .iter()
            .map(|(target, binding)| binding_row(target, binding, None, &unasked))
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if bindings.is_empty() {
        println!("no host declares an identity binding");
        return Ok(());
    }
    println!("{:<24} {:<16} {:<32} USER", "HOST", "KIND", "IDENTITY");
    for (target, binding) in &bindings {
        // The session user is optional in the declaration; a binding without
        // one says so rather than printing a dash.
        let user = match &binding.user {
            Some(user) => user.clone(),
            None => "(no user declared)".to_string(),
        };
        println!(
            "{:<24} {:<16} {:<32} {user}",
            target.name, binding.kind, binding.identity
        );
    }
    Ok(())
}
