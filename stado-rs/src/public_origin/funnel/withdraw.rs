//! Withdraw public HTTPS handlers without stopping their private backends.
use super::*;
use crate::primitives::failure::FailureCode;

async fn configuration(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let output = tailscale(target, &["serve", "status", "--json"], runner).await?;
    if !output.succeeded() {
        return Err(DeployError::unreachable(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "reading Funnel configuration failed")
        )));
    }
    serde_json::from_str(&output.stdout).map_err(|error| {
        DeployError(format!(
            "{}: invalid serve configuration: {error}",
            target.name
        ))
        .stating(FailureCode::InfraDown)
    })
}

/// Every endpoint Tailscale's own serve status permits publicly. A status
/// whose shape is not Tailscale's is the host's failure, not a refusal.
fn public_endpoints(config: &Value) -> Result<Vec<String>, DeployError> {
    let Some(allow) = config.get("AllowFunnel") else {
        return Ok(Vec::new());
    };
    let allow = allow.as_object().ok_or_else(|| {
        DeployError("AllowFunnel is not an object".into()).stating(FailureCode::InfraDown)
    })?;
    let mut endpoints = Vec::new();
    for (endpoint, enabled) in allow {
        match enabled.as_bool() {
            Some(true) => endpoints.push(endpoint.clone()),
            Some(false) => {}
            None => {
                return Err(
                    DeployError(format!("invalid Funnel permission for {endpoint}"))
                        .stating(FailureCode::InfraDown),
                )
            }
        }
    }
    Ok(endpoints)
}

/// Remove every observed public HTTPS handler on this target, including
/// undeclared paths. Refuse unsupported publication shapes before changing any.
/// A second invocation reads current state again and safely finishes a partial withdrawal.
pub async fn withdraw(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let before = configuration(target, runner).await?;
    let endpoints = public_endpoints(&before)?;
    let mut handlers = Vec::new();
    for endpoint in &endpoints {
        // The node's own serve report names every endpoint it publishes, so
        // the port is read from that answer rather than checked against a
        // copy of Tailscale's permitted list.
        let unsupported = |cause: String| {
            DeployError(format!("unsupported public endpoint {endpoint}: {cause}"))
                .stating(FailureCode::Refused)
        };
        let Some((_, port)) = endpoint.rsplit_once(':') else {
            return Err(unsupported("it names no port".to_string()));
        };
        port.parse::<u16>()
            .map_err(|error| unsupported(error.to_string()))?;
        let paths = before.get("Web").and_then(|web| web.get(endpoint))
            .and_then(|web| web.get("Handlers")).and_then(Value::as_object)
            .filter(|paths| !paths.is_empty())
            .ok_or_else(|| DeployError(format!("{endpoint}: public publication has no HTTPS handlers; refusing to guess its shutdown operation")).stating(FailureCode::Refused))?;
        for path in paths.keys() {
            handlers.push((port.to_string(), path.clone()));
        }
    }
    for (port, path) in &handlers {
        let https = format!("--https={port}");
        let set_path = format!("--set-path={path}");
        let output = tailscale(
            target,
            &["funnel", "--bg", &https, &set_path, "off"],
            runner,
        )
        .await?;
        if !output.succeeded() {
            return Err(DeployError::unreachable(format!(
                "{}: withdrawing public {port}{path} failed: {}",
                target.name,
                host_channel::last_error_line(&output, "Funnel withdrawal failed")
            )));
        }
    }
    let after = configuration(target, runner).await?;
    let remaining = public_endpoints(&after)?;
    if !remaining.is_empty() {
        return Err(DeployError(format!(
            "{} still permits public Funnel access at {}; withdrawal is incomplete",
            target.name,
            remaining.join(", ")
        ))
        .stating(FailureCode::InfraDown));
    }
    Ok(serde_json::json!({
        "target": target.name,
        "public_endpoints_before": endpoints,
        "removed_handlers": handlers,
        "public_endpoints_after": remaining,
        "status": "withdrawn"
    }))
}
