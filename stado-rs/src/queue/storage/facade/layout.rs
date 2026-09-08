//! The storage-layout marker: the one object that proves a store belongs to
//! this product at a schema version this build understands.

use crate::queue::StorageError;

use super::JobStorage;

const LAYOUT_PATH: &str = "system/storage-layout.json";

fn validate_layout_document(raw: &str) -> Result<(), StorageError> {
    let document: serde_json::Value = serde_json::from_str(raw)?;
    let product = document.get("product").and_then(serde_json::Value::as_str);
    let version = document
        .get("schema_version")
        .and_then(serde_json::Value::as_u64);
    if product != Some("stado") {
        return Err(StorageError::Other(
            "storage layout marker belongs to another product".to_string(),
        ));
    }
    if version != Some(u64::from(super::STORAGE_LAYOUT_VERSION)) {
        return Err(StorageError::Other(format!(
            "unsupported storage layout schema_version {}; expected {}",
            version
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string()),
            super::STORAGE_LAYOUT_VERSION
        )));
    }
    Ok(())
}

impl JobStorage {
    pub(super) async fn ensure_layout(&self) -> Result<(), StorageError> {
        if let Some(raw) = self.backend.download_text(LAYOUT_PATH).await? {
            return validate_layout_document(&raw);
        }
        let body = serde_json::to_string(&serde_json::json!({
            "product": "stado",
            "schema_version": super::STORAGE_LAYOUT_VERSION,
        }))?;
        if self
            .backend
            .upload_text_if_absent(LAYOUT_PATH, &body)
            .await?
        {
            return Ok(());
        }
        let raw = self
            .backend
            .download_text(LAYOUT_PATH)
            .await?
            .ok_or_else(|| {
                StorageError::Other(
                    "storage layout marker disappeared after concurrent creation".to_string(),
                )
            })?;
        validate_layout_document(&raw)
    }
}
