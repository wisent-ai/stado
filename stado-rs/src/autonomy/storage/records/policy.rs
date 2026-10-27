//! The autonomy policy object: read, validated, and applied atomically
//! against the version the operator saw.

use crate::autonomy::policy::AutonomyPolicy;
use crate::autonomy::storage::POLICY_PATH;
use crate::queue::{JobStorage, StorageError};

/// The operator's policy, or `None` when none was ever written: autonomy has
/// no limits of its own to run under. A stored policy that cannot be read is
/// refused naming its object and what serde could not read in it, so the
/// tick's log and `stado optimize status` say which field to state; it stays
/// a JSON error (`de::Error::custom`, a data error), which the CLI states as
/// refused input, as it states every malformed document.
pub async fn load_policy(store: &JobStorage) -> Result<Option<AutonomyPolicy>, StorageError> {
    let Some(raw) = store.download_text(POLICY_PATH).await? else {
        return Ok(None);
    };
    let policy: AutonomyPolicy = serde_json::from_str(&raw).map_err(|error| {
        StorageError::Json(<serde_json::Error as serde::de::Error>::custom(format!(
            "{POLICY_PATH} is not an autonomy policy this Stado reads: {error}"
        )))
    })?;
    policy
        .validate()
        .map_err(|reason| StorageError::Other(format!("{POLICY_PATH} is refused: {reason}")))?;
    Ok(Some(policy))
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
