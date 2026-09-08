//! The reconcile pass: what has to hold before the host is touched, which
//! storage half the operator asked for, and the report the run leaves behind.

use serde_json::Value;

use super::{install_timeout, parse_fields, report};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::stream::schema::DisplayStream;
use crate::targets::ComputeTarget;

mod library_bind;
mod script;

/// Reconcile the host to its declaration: packages, screen, session, Sunshine,
/// units. Idempotent — an installed host reports what it already had.
///
/// `bus_id` is the PCI address of the declared board, resolved by the caller
/// from the probe, because Xorg addresses a card by bus id and the declaration
/// names it by driver UUID.
pub async fn install(
    target: &ComputeTarget,
    declaration: &DisplayStream,
    bus_id: &str,
    provision_library: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    declaration
        .validate(&format!("targets[{}].display_stream", target.name))
        .map_err(DeployError)?;
    let (width, height) = declaration
        .dimensions()
        .ok_or_else(|| DeployError("resolution has no dimensions".to_string()))?;
    let steam_packages = if declaration.steam {
        "steam-installer"
    } else {
        ""
    };
    // Reshaping a host's storage is not something to do silently, so the two
    // halves are separate: without the flag a library with no room is refused
    // and the largest mounts are named; with it, the declared path becomes a
    // bind mount on the largest disk-backed filesystem, the same shape this
    // host already uses for agent staging.
    let library_block = if provision_library {
        library_bind::LIBRARY_BIND
    } else {
        ""
    };
    let script = script::install_script(
        declaration,
        bus_id,
        library_block,
        steam_packages,
        width,
        height,
    );
    let output =
        host_channel::run_script_with_timeout(target, &script, install_timeout(), runner).await?;
    let mut body = report(target, &output, "installed");
    if let Some(map) = body.as_object_mut() {
        map.insert(
            "fields".to_string(),
            Value::Object(parse_fields(&output.stdout)),
        );
    }
    Ok(body)
}
