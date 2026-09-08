//! The public entry point that sequences one delivery: plan, canonical
//! target, approved home, preflight, transfer, commit, receipt.

use serde_json::{json, Value};

use crate::deploy::{host_channel, DeployError, Runner};

use super::plan::plan;
use super::script::DELIVERED_STATUS;
use super::stages::{commit, preflight, transfer};

/// Deliver one local source to one canonical registry target.
pub async fn deliver_host(
    target_name: &str,
    source: &str,
    destination: &str,
    file_list: Option<&str>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    // Local shape and destination policy are decided before registry or host
    // contact. Host-dependent guards then run before rsync transfers a byte.
    let plan = plan(source, destination, file_list)?;
    let target = host_channel::canonical_target(target_name).await?;
    let home = host_channel::remote_home(&target, runner).await?;
    if home
        .bytes()
        .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')))
    {
        return Err(DeployError(format!(
            "{}: the approved account home cannot be represented safely by the rsync transport",
            target.name
        )));
    }
    let (absolute_destination, stage) = preflight(&target, &home, &plan, runner).await?;
    transfer(&target, &stage, &plan, runner).await?;
    commit(&target, &home, &absolute_destination, &stage, &plan, runner).await?;
    Ok(json!({
        "schema": "stado.host-delivery-receipt.v1",
        "target": target.name,
        "source": plan.source,
        "destination": format!("$HOME/{}", plan.destination),
        "kind": plan.kind.word(),
        "selection": if plan.file_list.is_some() { "nul-file-list" } else { "complete-source" },
        "status": DELIVERED_STATUS,
    }))
}
