use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::service_resolution;
use crate::targets::{self, RegistryStore};

use crate::cli::CmdError;

pub(in crate::cli::resolver) mod document;
pub(in crate::cli::resolver) mod source;

const SNAPSHOT_LIMIT: usize = 1024 * 1024;
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPayload {
    store_version: String,
    document: Value,
}

fn validate_snapshot(payload: SnapshotPayload) -> Result<(Value, String, u64), String> {
    targets::validate_registry(&payload.document).map_err(|error| error.to_string())?;
    let directory = service_resolution::directory(&payload.document)?
        .ok_or_else(|| "registry.service_directory is required".to_string())?;
    Ok((
        payload.document,
        payload.store_version,
        directory.generation,
    ))
}

/// Read the selected registry store without requiring a service directory.
/// Each caller validates the document for the surface that consumes it.
pub(crate) async fn read_local_document(
    store: &RegistryStore,
) -> Result<(Value, String), CmdError> {
    let blob = store
        .read_versioned()
        .await
        .map_err(|error| {
            let mut error = CmdError::from(error);
            if let Some(message) = error.message.as_mut() {
                message.insert_str(0, "registry read failed: ");
            }
            error
        })?
        .ok_or_else(|| crate::cli::registry::registry_absent(store.location()))?;
    let document: Value = serde_json::from_str(&blob.content).map_err(|error| {
        CmdError::click(format!("invalid registry JSON: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    Ok((document, blob.version))
}

pub(crate) async fn read_local_snapshot(
    store: &RegistryStore,
) -> Result<(Value, String, u64), CmdError> {
    let (document, store_version) = read_local_document(store).await?;
    validate_snapshot(SnapshotPayload {
        store_version,
        document,
    })
    .map_err(CmdError::click)
}

pub(super) async fn emit_snapshot() -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let (document, store_version, _) = read_local_snapshot(&store).await?;
    println!(
        "{}",
        serde_json::to_string(&SnapshotPayload {
            store_version,
            document,
        })?
    );
    Ok(())
}
