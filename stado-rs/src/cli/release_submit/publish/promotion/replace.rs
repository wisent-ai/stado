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
    let directory = crate::service_resolution::directory(document)?.ok_or_else(|| {
        CmdError::click("service directory disappeared")
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let route = directory.services.get(logical_service).ok_or_else(|| {
        CmdError::click("release product service disappeared")
            .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    if route.active_host != target {
        return Err(CmdError::refused(format!(
            "release product service {logical_service:?} is active on {}, not {target}",
            route.active_host
        )));
    }
    // A fixed service names its unit in `managed_service`; a placement-backed
    // one must leave it absent and declares its unit per host in the profile
    // (`placement_profiles[].hosts.<target>.units.<service>`), which `service
    // release` resolves from the logical name. Reading only the first refused
    // every release of a product whose service is a unit of its placement
    // profile, with "has no managed service".
    let managed_service = match route.managed_service.clone() {
        Some(unit) => unit,
        None => {
            let profile = route.placement_profile.as_deref().ok_or_else(|| {
                CmdError::click(format!(
                    "release product service {logical_service:?} names neither a managed service \
                     nor a placement profile"
                ))
                .stating(crate::primitives::failure::FailureCode::Config)
            })?;
            let declared = document
                .get("placement_profiles")
                .and_then(Value::as_array)
                .and_then(|profiles| {
                    profiles
                        .iter()
                        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(profile))
                })
                .and_then(|entry| {
                    entry.pointer(&format!("/hosts/{target}/units/{logical_service}"))
                })
                .is_some();
            if !declared {
                return Err(CmdError::click(format!(
                    "release product service {logical_service:?} is placed by profile {profile:?}, \
                     which declares no {logical_service:?} unit on {target}"
                ))
                .stating(crate::primitives::failure::FailureCode::Config));
            }
            logical_service.to_string()
        }
    };
    let endpoint = route.endpoints.get(target).ok_or_else(|| {
        CmdError::click("release product service has no target endpoint")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let mut readiness = url::Url::parse(&endpoint.url).map_err(|error| {
        CmdError::click(format!("invalid release service endpoint: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    readiness.set_path(readiness_path);
    readiness.set_query(None);
    readiness.set_fragment(None);
    Ok((managed_service, readiness.to_string()))
}
