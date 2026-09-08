//! `identity relay-apple-challenge`: capture on the holder, store on the worker.

use anyhow::Result;
use serde_json::{json, Value};

use super::super::{verified_bindings, Verification};
use crate::cli::identity::APPLE_ACCOUNT;
use crate::cli::CmdError;
use crate::targets::load_registry_auto;

/// Capture a trusted-device code on the verified holder and put it into the
/// Weles capability broker on the host executing this command.
pub async fn relay_apple_challenge(
    identity: String,
    authorization_id: String,
    preflight: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if identity.trim().is_empty() {
        return Err(CmdError::click("an Apple account identity is required"));
    }
    if uuid::Uuid::parse_str(&authorization_id).is_err() {
        return Err(CmdError::click("--authorization-id must be a UUID"));
    }
    let registry = load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let Verification { rows, .. } = verified_bindings(&registry, APPLE_ACCOUNT, &identity).await;
    let holder = rows.iter().find(|row| {
        row.get("observed").and_then(Value::as_bool) == Some(true)
            && row.get("drivable_session").and_then(Value::as_bool) == Some(true)
    });
    let Some(holder) = holder else {
        let observed = rows
            .iter()
            .filter(|row| row.get("observed").and_then(Value::as_bool) == Some(true))
            .filter_map(|row| {
                let host = row.get("host")?.as_str()?;
                let user = row
                    .get("user")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown-user");
                Some(format!("{host}/{user}"))
            })
            .collect::<Vec<_>>();
        if observed.is_empty() {
            return Err(CmdError::click(format!(
                "no verified host holds apple-account {identity}"
            )));
        }
        return Err(CmdError::click(format!(
            "apple-account {identity} is held on {}, but none of those Apple challenge sessions is drivable",
            observed.join(", ")
        )));
    };
    let holder_name = holder
        .get("host")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("the Apple identity report names no holder"))?;
    let holder_user = holder
        .get("user")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("the Apple identity holder names no macOS user"))?;
    let holder_target = registry
        .targets
        .iter()
        .find(|target| target.name == holder_name)
        .ok_or_else(|| CmdError::click("the Apple identity holder left the registry"))?;

    let destinations = registry
        .targets
        .iter()
        .filter(|target| crate::deploy::host_channel::target_is_this_host(target))
        .collect::<Vec<_>>();
    let [destination] = destinations.as_slice() else {
        return Err(CmdError::click(format!(
            "the current machine resolves to {} registry targets; exactly one is required",
            destinations.len()
        )));
    };
    let password = super::service::host_sudo_password(holder_target).await?;
    let runner = crate::deploy::production_runner();
    let resource = format!("challenge:apple/{authorization_id}");
    let broker = crate::deploy::host_capability::resolve(
        destination,
        &crate::deploy::weles_browser_task::weles_api_broker_files(),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    if preflight {
        crate::deploy::host_gui_automation::preflight_apple_challenge(
            holder_target,
            holder_user,
            password.as_deref(),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.0))?;
        let receipt = json!({
            "status": "ready",
            "identity": identity,
            "holder": holder_name,
            "user": holder_user,
            "destination": destination.name,
            "resource": resource,
        });
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&receipt).unwrap_or_default()
            );
        } else {
            println!(
                "Apple challenge relay is ready from {holder_name}/{holder_user} to {}",
                destination.name
            );
        }
        return Ok(());
    }

    let mut code = crate::deploy::host_gui_automation::capture_apple_challenge(
        holder_target,
        holder_user,
        &authorization_id,
        90,
        password.as_deref(),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    let stored = crate::deploy::host_capability::apple_challenge_put(
        destination,
        &broker,
        &resource,
        &code,
        &runner,
    )
    .await;
    code.clear();
    stored.map_err(|error| CmdError::click(error.0))?;

    let receipt = json!({
        "status": "stored",
        "identity": identity,
        "holder": holder_name,
        "user": holder_user,
        "destination": destination.name,
        "resource": resource,
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&receipt).unwrap_or_default()
        );
    } else {
        println!(
            "stored Apple challenge from {holder_name}/{holder_user} for {}",
            destination.name
        );
    }
    Ok(())
}
