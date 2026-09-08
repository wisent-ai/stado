//! The autonomy policy object: read, validated, and applied atomically
//! against the version the operator saw.

use crate::autonomy::policy::AutonomyPolicy;
use crate::autonomy::storage::POLICY_PATH;
use crate::queue::{JobStorage, StorageError};

pub async fn load_policy(store: &JobStorage) -> Result<AutonomyPolicy, StorageError> {
    let Some(raw) = store.download_text(POLICY_PATH).await? else {
        return Ok(AutonomyPolicy::default());
    };
    let policy: AutonomyPolicy = serde_json::from_str(&raw)?;
    policy.validate().map_err(StorageError::Other)?;
    Ok(policy)
}

pub async fn write_policy(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    expected_version: Option<&str>,
) -> Result<String, StorageError> {
    policy.validate().map_err(StorageError::Other)?;
    let content = serde_json::to_string(policy)?;
    match expected_version {
        Some(version) => {
            store
                .compare_and_swap_text(POLICY_PATH, version, &content)
                .await
        }
        None => {
            if store.create_text_if_absent(POLICY_PATH, &content).await? {
                let stored = store
                    .read_text_versioned(POLICY_PATH)
                    .await?
                    .ok_or_else(|| StorageError::NotFound(POLICY_PATH.to_string()))?;
                Ok(stored.version)
            } else {
                Err(StorageError::StorageConflict(
                    "autonomy policy already exists; expected_version is required".to_string(),
                ))
            }
        }
    }
}

pub async fn load_policy_versioned(
    store: &JobStorage,
) -> Result<Option<crate::queue::VersionedText>, StorageError> {
    store.read_text_versioned(POLICY_PATH).await
}
