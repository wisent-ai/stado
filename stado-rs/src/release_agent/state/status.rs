//! The rollout status document one host publishes for the fleet to read back.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::records::{HostReleaseState, RolloutPhase, STATUS_SCHEMA};

#[derive(Debug, Serialize)]
struct PublishedStatus<'a> {
    schema_version: u32,
    product: &'a str,
    target: &'a str,
    rollout_generation: u64,
    phase: RolloutPhase,
    active_version: Option<&'a str>,
    active_sha256: Option<&'a str>,
    previous_version: Option<&'a str>,
    detail: &'a str,
    updated_at: DateTime<Utc>,
}

/// Canonical object URI of one host's rollout status, inside this
/// deployment's own namespace and its declared `system/` prefix.
///
/// The literal `stado://system/...` this replaced named a namespace no grant
/// declares, so every publish answered 401. Resolving through `ObjectRef`
/// keeps writer and reader on one path whether they reach the store through
/// the object API or read the co-located disk directly.
pub fn release_status_uri(product: &str, target: &str) -> String {
    let namespace = crate::config::wc_stado_storage_namespace();
    format!("stado://{namespace}/system/release-status/{product}/{target}.json")
}

pub(crate) async fn publish_status(state: &HostReleaseState) -> Result<(), String> {
    let status = PublishedStatus {
        schema_version: STATUS_SCHEMA,
        product: &state.product,
        target: &state.target,
        rollout_generation: state.rollout_generation,
        phase: state.phase,
        active_version: state.active.as_ref().map(|record| record.version.as_str()),
        active_sha256: state
            .active
            .as_ref()
            .map(|record| record.artifact_sha256.as_str()),
        previous_version: state
            .previous
            .as_ref()
            .map(|record| record.version.as_str()),
        detail: &state.detail,
        updated_at: state.updated_at,
    };
    let temporary = tempfile::NamedTempFile::new()
        .map_err(|error| format!("cannot create release status staging: {error}"))?;
    std::fs::write(
        temporary.path(),
        serde_json::to_vec(&status).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write release status staging: {error}"))?;
    crate::cli::storage::store_object(
        &release_status_uri(&state.product, &state.target),
        &temporary.path().display().to_string(),
        "application/json",
        false,
    )
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Publish the committed outcome of a generic managed-service activation into
/// the same status document `stado release status` reads.
// The status document is a flat wire contract; keeping its fields explicit
// makes accidental schema changes visible at every publication call.
#[allow(clippy::too_many_arguments)]
pub async fn publish_service_release_status(
    product: &str,
    target: &str,
    rollout_generation: u64,
    phase: RolloutPhase,
    active_version: Option<&str>,
    active_sha256: Option<&str>,
    previous_version: Option<&str>,
    detail: &str,
) -> Result<(), String> {
    let status = PublishedStatus {
        schema_version: STATUS_SCHEMA,
        product,
        target,
        rollout_generation,
        phase,
        active_version,
        active_sha256,
        previous_version,
        detail,
        updated_at: Utc::now(),
    };
    let temporary = tempfile::NamedTempFile::new()
        .map_err(|error| format!("cannot create service release status staging: {error}"))?;
    std::fs::write(
        temporary.path(),
        serde_json::to_vec(&status).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write service release status staging: {error}"))?;
    crate::cli::storage::store_object(
        &release_status_uri(product, target),
        &temporary.path().display().to_string(),
        "application/json",
        false,
    )
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}
