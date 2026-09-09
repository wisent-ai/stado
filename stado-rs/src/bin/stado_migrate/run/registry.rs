//! Registry steps of a coordinator migration: the single atomic flip of
//! the active entry, then the read-back verification.
//!
//! The registry flip goes through the validated compare-and-swap write
//! path shared with `stado registry push`, never a hand-rolled upload.

use serde_json::Value;
use stado::cli::registry::{commit_document, fetch_document};
use stado::deploy::bootstrap::ssh_argv;
use stado::deploy::{CommandSpec, Runner};

use crate::plan::MigrationPlan;

use super::label;

/// The single atomic registry mutation of the whole migration: one
/// compare-and-swapped document where exactly the target is active.
///
/// Pure — "exactly this coordinator is active" is a function of the
/// coordinator list it is applied to — so a writer that landed between the
/// read and the write is answered by re-applying the flip to their document
/// instead of erasing it.
pub(super) async fn flip_registry(plan: &MigrationPlan) -> Result<String, String> {
    let generation = commit_document(|current| {
        let mut document = current.clone();
        let entries = document
            .get_mut("coordinators")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                stado::cli::CmdError::click("registry document has no coordinators array")
            })?;
        for entry in entries.iter_mut() {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            entry["active"] = Value::Bool(name == plan.to_name);
        }
        Ok(document)
    })
    .await
    .map_err(|exc| exc.to_string())?;
    println!(
        "[registry] active moved '{}' -> '{}' (generation {generation})",
        plan.from_name, plan.to_name
    );
    Ok(generation)
}

/// Read the registry back and require the target as the only active entry;
/// then report the remote service state without enforcing it, since launchd
/// visibility can lag the bootstrap by a moment.
pub(super) async fn verify(runner: &Runner, plan: &MigrationPlan) -> Result<(), String> {
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    let active: Vec<String> = document
        .get("coordinators")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| entry.get("active").and_then(Value::as_bool) == Some(true))
                .filter_map(|entry| {
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    if active != vec![plan.to_name.clone()] {
        return Err(format!(
            "registry verification failed: active coordinators are now: {}",
            active.join(", ")
        ));
    }
    let check = format!("launchctl print gui/$(id -u)/{}", label(&plan.to_name));
    let out = runner(CommandSpec::new(ssh_argv(&plan.to_host, &check))).await?;
    if out.ok() {
        println!(
            "[verify] {} reports the coordinator service loaded",
            plan.to_host
        );
    } else {
        println!(
            "[verify] warning: service not visible on {} yet: {}",
            plan.to_host,
            out.detail()
        );
    }
    Ok(())
}
