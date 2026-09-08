//! `identity issue-apple-capabilities`: the three grants one Apple login redeems.

use anyhow::Result;
use serde_json::json;

use crate::cli::CmdError;
use crate::targets::load_registry_auto;

/// Issue the three authorization-bound Apple login capabilities in the broker
/// on the worker that will redeem them.
pub async fn issue_apple_capabilities(
    target_name: String,
    agent: String,
    authorization_id: String,
    ttl_seconds: u64,
    json_output: bool,
) -> Result<(), CmdError> {
    if uuid::Uuid::parse_str(&authorization_id).is_err() {
        return Err(CmdError::click("--authorization-id must be a UUID"));
    }
    if agent.trim().is_empty() || agent.trim() != agent {
        return Err(CmdError::click("--agent must be a non-empty exact name"));
    }
    if !(60..=3600).contains(&ttl_seconds) {
        return Err(CmdError::click("--ttl-seconds must be between 60 and 3600"));
    }
    let registry = load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let target = registry
        .targets
        .iter()
        .find(|target| target.name == target_name)
        .ok_or_else(|| CmdError::click(format!("unknown target {target_name}")))?;
    let runner = crate::deploy::production_runner();
    let broker = crate::deploy::host_capability::resolve(
        target,
        &crate::deploy::weles_browser_task::weles_api_broker_files(),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    let ttl = ttl_seconds.to_string();

    let email_purpose = "weles.browser.fill";
    let email_resource = "origin:https://idmsa.apple.com/email";
    let email_id = crate::deploy::host_capability::issue(
        target,
        &broker,
        &crate::deploy::host_capability::Issuance {
            agent: &agent,
            purpose: email_purpose,
            resource: email_resource,
            capability_target: "weles",
            ttl_seconds: &ttl,
            max_uses: "1",
            authorization_id: Some(&authorization_id),
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    let password_purpose = "weles.browser.fill";
    let password_resource = "origin:https://idmsa.apple.com/password";
    let password_id = crate::deploy::host_capability::issue(
        target,
        &broker,
        &crate::deploy::host_capability::Issuance {
            agent: &agent,
            purpose: password_purpose,
            resource: password_resource,
            capability_target: "weles",
            ttl_seconds: &ttl,
            max_uses: "1",
            authorization_id: Some(&authorization_id),
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    let challenge_purpose = "weles.apple.2fa";
    let challenge_resource = format!("challenge:apple/{authorization_id}");
    let challenge_id = crate::deploy::host_capability::issue(
        target,
        &broker,
        &crate::deploy::host_capability::Issuance {
            agent: &agent,
            purpose: challenge_purpose,
            resource: &challenge_resource,
            capability_target: "weles",
            ttl_seconds: &ttl,
            max_uses: "1",
            authorization_id: Some(&authorization_id),
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;

    let capability_ref = |capability_id: String, purpose: &str, resource: &str| {
        json!({
            "capability_id": capability_id,
            "purpose": purpose,
            "resource": resource,
            "target": "weles",
            "authorization_id": authorization_id,
        })
    };
    let receipt = json!({
        "status": "issued",
        "target": target.name,
        "authorization_id": authorization_id,
        "capabilities": {
            "email": capability_ref(email_id, email_purpose, email_resource),
            "password": capability_ref(password_id, password_purpose, password_resource),
            "two_factor": {
                "mode": "capability",
                "capability": capability_ref(
                    challenge_id,
                    challenge_purpose,
                    &challenge_resource
                ),
            },
        },
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&receipt).unwrap_or_default()
        );
    } else {
        println!(
            "issued one Apple login authorization on {} for {agent}",
            target.name
        );
    }
    Ok(())
}
