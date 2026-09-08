use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::service_resolution;
use crate::targets::{self, RegistryStore};

use crate::cli::CmdError;

pub(in crate::cli::resolver) mod document;
pub(in crate::cli::resolver) mod source;

const SNAPSHOT_LIMIT: usize = 1024 * 1024;
/// Maximum wall time for one authority snapshot.
///
/// OpenSSH's connect and keepalive settings do not bound a remote command that
/// stays alive without producing a snapshot. Without this deadline one stuck
/// `resolver snapshot` blocks refresh forever while the local listener keeps
/// accepting requests it can no longer answer.
const AUTHORITY_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

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

pub(crate) async fn read_local_snapshot(
    store: &RegistryStore,
) -> Result<(Value, String, u64), String> {
    let blob = store
        .read_versioned()
        .await
        .map_err(|error| format!("registry read failed: {error}"))?
        .ok_or_else(|| format!("no registry document at {}", store.location()))?;
    let document: Value = serde_json::from_str(&blob.content)
        .map_err(|error| format!("invalid registry JSON: {error}"))?;
    validate_snapshot(SnapshotPayload {
        store_version: blob.version,
        document,
    })
}

pub(super) async fn emit_snapshot() -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let (document, store_version, _) =
        read_local_snapshot(&store).await.map_err(CmdError::click)?;
    println!(
        "{}",
        serde_json::to_string(&SnapshotPayload {
            store_version,
            document,
        })?
    );
    Ok(())
}
