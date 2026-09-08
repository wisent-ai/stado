//! The deployment receipt a converged rollout leaves, written once and
//! compared by identity when a retry writes it again.

use serde_json::Value;

use crate::cli::release_submit::run::source::queue_immutable;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

fn deployment_receipt_identity(bytes: &[u8]) -> Result<Value, CmdError> {
    let mut receipt: Value = serde_json::from_slice(bytes)?;
    let object = receipt
        .as_object_mut()
        .ok_or_else(|| CmdError::click("deployment receipt is not an object"))?;
    if !matches!(object.remove("completed_at"), Some(Value::String(_))) {
        return Err(CmdError::click(
            "deployment receipt has no completed_at timestamp",
        ));
    }
    Ok(receipt)
}

pub(super) async fn queue_deployment_receipt(path: &str, bytes: &[u8]) -> Result<(), CmdError> {
    let original_error = match queue_immutable(path, bytes).await {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let Some(existing) = store
        .read_bytes(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    else {
        return Err(original_error);
    };
    if deployment_receipt_identity(&existing)? == deployment_receipt_identity(bytes)? {
        Ok(())
    } else {
        Err(original_error)
    }
}
