use super::*;

mod decisions;
mod fence;
mod state;

pub(in crate::deploy::host_storage_reconcile) use decisions::*;
pub(in crate::deploy::host_storage_reconcile) use fence::*;
pub(in crate::deploy::host_storage_reconcile) use state::*;

pub(super) fn read_transaction_receipt(transaction: &str) -> Result<Value, DeployError> {
    let path = transaction_directory(transaction)?.join("receipt.json");
    let metadata = std::fs::symlink_metadata(&path).map_err(DeployError::io(format!(
        "cannot inspect {}",
        path.display()
    )))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(DeployError(format!(
            "transaction receipt is not a regular file: {}",
            path.display()
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let receipt: Value = serde_json::from_slice(
        &std::fs::read(&path)
            .map_err(DeployError::io(format!("cannot read {}", path.display())))?,
    )
    .map_err(|error| {
        DeployError(format!("transaction receipt is invalid: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    if receipt.get("schema").and_then(Value::as_str) != Some("stado.storage-root-reconcile.v2")
        || receipt.get("transaction").and_then(Value::as_str) != Some(transaction)
    {
        return Err(DeployError(
            "transaction receipt belongs to another reconciliation".to_string(),
        )
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    Ok(receipt)
}

fn receipt_evidence_reference(
    receipt: &Value,
    field: &str,
    label: &str,
) -> Result<ImmutableEvidenceReference, DeployError> {
    serde_json::from_value(receipt.get(field).cloned().ok_or_else(|| {
        DeployError(format!("receipt omitted {label} reference"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?)
    .map_err(|error| {
        DeployError(format!("receipt {label} reference is invalid: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}

pub(super) fn report(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    receipt: Value,
    fence: Option<&LifecycleFence>,
) -> Result<Value, DeployError> {
    let mut report = host_channel::base_report(target);
    report.insert("transaction".to_string(), json!(transaction));
    report.insert("phase".to_string(), json!(phase));
    report.insert("receipt".to_string(), receipt);
    report.insert(
        "lifecycle_fence".to_string(),
        match fence {
            Some(fence) => serde_json::to_value(fence)
                .map_err(|error| DeployError(format!("cannot report lifecycle fence: {error}")))?,
            None => Value::Null,
        },
    );
    report.insert("status".to_string(), json!("ok"));
    Ok(Value::Object(report))
}
