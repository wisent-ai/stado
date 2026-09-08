//! The inventory snapshot: sealed once under its own id, then published as
//! the latest one a reader may trust.

use crate::autonomy::model::InventorySnapshot;
use crate::autonomy::storage::{INVENTORY_LATEST_PATH, INVENTORY_PREFIX};
use crate::queue::{JobStorage, StorageError};

pub async fn publish_inventory(
    store: &JobStorage,
    snapshot: &InventorySnapshot,
) -> Result<(), StorageError> {
    let content = serde_json::to_string(snapshot)?;
    let path = format!("{INVENTORY_PREFIX}/{}.json", snapshot.snapshot_id);
    if !store.create_text_if_absent(&path, &content).await? {
        return Err(StorageError::StorageConflict(format!(
            "inventory snapshot {} already exists",
            snapshot.snapshot_id
        )));
    }
    store.upload_text(INVENTORY_LATEST_PATH, &content).await
}

pub async fn load_latest_inventory(
    store: &JobStorage,
) -> Result<Option<InventorySnapshot>, StorageError> {
    let Some(raw) = store.download_text(INVENTORY_LATEST_PATH).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&raw)?))
}
