//! The Probierz signing identity resolved through Brama's Skarbiec routes.

use serde_json::Value;

use super::brama::brama_skarbiec_context;
use super::installer::PROBIERZ_AGENT_RESOURCE;
use super::report::command_failure;
use crate::deploy::{host_channel, DeployError};
use crate::targets::ComputeTarget;

pub(crate) struct ProbierzAgentCredential {
    pub(crate) item: String,
    pub(crate) field: String,
    pub(crate) secret: String,
}

/// The Probierz agent identity the runner signs Kronika requests with, resolved
/// through Brama's own Skarbiec broker rather than named here.
///
/// `target` is the host whose Brama installation holds that broker, which is
/// NOT always the runner's host -- see [`super::brama::brama_identity_host`].
/// The route is `agent:probierz`, and the item and field behind it are whatever
/// Brama's capability-routes table says today: a credential that is retagged or
/// renamed is still found, and this function never has to be edited to follow
/// it. Resolving it anywhere other than beside Brama would mean reading a second
/// copy of one fleet identity out of a second vault, and the two would drift.
pub(crate) async fn kronika_agent_credential(
    target: &ComputeTarget,
) -> Result<ProbierzAgentCredential, DeployError> {
    let context = brama_skarbiec_context(target).await?;
    let runner = context.runner;
    let skarbiec = context.skarbiec;
    let vault = context.vault;
    let routes = context.routes;
    let gnupg = context.gnupg;
    let program_path = "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    // Read the capability routes from Skarbiec's declared route capability.
    // The vault resolves self-declared identities at read time.
    let resolved = host_channel::run_program(
        target,
        &[
            "/usr/bin/env",
            &vault,
            &routes,
            &gnupg,
            program_path,
            &skarbiec,
            "route",
            "resolve",
        ],
        &runner,
    )
    .await?;
    if !resolved.ok() {
        return Err(DeployError(format!(
            "{}: cannot resolve {PROBIERZ_AGENT_RESOURCE} through Skarbiec: {}",
            target.name,
            command_failure(&resolved, "capability route lookup failed")
        )));
    }
    let document: Value = serde_json::from_str(&resolved.stdout)
        .map_err(|error| DeployError(format!("Skarbiec route report is invalid: {error}")))?;
    let route = document
        .get("routes")
        .and_then(Value::as_array)
        .and_then(|routes| {
            routes.iter().find(|route| {
                route.get("resource").and_then(Value::as_str) == Some(PROBIERZ_AGENT_RESOURCE)
            })
        })
        .ok_or_else(|| {
            DeployError(format!(
                "Skarbiec maps no credential for {PROBIERZ_AGENT_RESOURCE}; declare one with \
                 `skarbiec route declare --resource {PROBIERZ_AGENT_RESOURCE} --item <item> \
                 --field <field> --reason <text>` beside that host's Brama"
            ))
        })?;
    if route.get("item_present") != Some(&Value::Bool(true))
        || route.get("field_present") != Some(&Value::Bool(true))
    {
        return Err(DeployError(format!(
            "Skarbiec route {PROBIERZ_AGENT_RESOURCE} does not resolve to a readable field"
        )));
    }
    let item = route
        .get("item")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .map(str::to_string)
        .ok_or_else(|| DeployError("Probierz agent route has no valid item".to_string()))?;
    let field = route
        .get("field")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .map(str::to_string)
        .ok_or_else(|| DeployError("Probierz agent route has no valid field".to_string()))?;
    let read = host_channel::run_program(
        target,
        &[
            "/usr/bin/env",
            &vault,
            &routes,
            &gnupg,
            program_path,
            &skarbiec,
            "get",
            &item,
            "--field",
            &field,
        ],
        &runner,
    )
    .await?;
    if !read.ok() {
        return Err(DeployError(format!(
            "{}: cannot read {PROBIERZ_AGENT_RESOURCE} through its Skarbiec route: {}",
            target.name,
            command_failure(&read, "routed credential read failed")
        )));
    }
    let secret = read.stdout.trim();
    if secret.is_empty() || secret.chars().any(char::is_control) {
        return Err(DeployError(format!(
            "Skarbiec route {PROBIERZ_AGENT_RESOURCE} returned an empty or malformed credential"
        )));
    }
    Ok(ProbierzAgentCredential {
        item,
        field,
        secret: secret.to_string(),
    })
}
