//! The replace strategy's own two questions: whether a target has committed
//! the exact coordinate, and which managed service and readiness URL it is.

use serde::Deserialize;
use serde_json::Value;

use crate::cli::storage;
use crate::cli::CmdError;

#[derive(Deserialize)]
struct ReplaceRolloutStatus {
    rollout_generation: u64,
    phase: crate::release_agent::RolloutPhase,
    active_version: Option<String>,
    active_sha256: Option<String>,
}

pub(super) async fn replace_status_exact(
    product: &str,
    target: &str,
    generation: u64,
    version: &str,
    artifact_sha256: &str,
) -> bool {
    let uri = crate::release_agent::release_status_uri(product, target);
    let Ok(bytes) = storage::fetch_object(&uri).await else {
        return false;
    };
    let Ok(status) = serde_json::from_slice::<ReplaceRolloutStatus>(&bytes) else {
        return false;
    };
    status.rollout_generation == generation
        && status.phase == crate::release_agent::RolloutPhase::Committed
        && status.active_version.as_deref() == Some(version)
        && status.active_sha256.as_deref() == Some(artifact_sha256)
}

pub(super) fn replace_service(
    document: &Value,
    logical_service: &str,
    target: &str,
    readiness_path: &str,
) -> Result<(String, String), CmdError> {
    let directory = crate::service_resolution::directory(document)?
        .ok_or_else(|| CmdError::click("service directory disappeared"))?;
    let route = directory
        .services
        .get(logical_service)
        .ok_or_else(|| CmdError::click("release product service disappeared"))?;
    if route.active_host != target {
        return Err(CmdError::click(format!(
            "release product service {logical_service:?} is active on {}, not {target}",
            route.active_host
        )));
    }
    let managed_service = route.managed_service.clone().ok_or_else(|| {
        CmdError::click(format!(
            "release product service {logical_service:?} has no managed service"
        ))
    })?;
    let endpoint = route
        .endpoints
        .get(target)
        .ok_or_else(|| CmdError::click("release product service has no target endpoint"))?;
    let mut readiness = url::Url::parse(&endpoint.url)
        .map_err(|error| CmdError::click(format!("invalid release service endpoint: {error}")))?;
    readiness.set_path(readiness_path);
    readiness.set_query(None);
    readiness.set_fragment(None);
    Ok((managed_service, readiness.to_string()))
}
