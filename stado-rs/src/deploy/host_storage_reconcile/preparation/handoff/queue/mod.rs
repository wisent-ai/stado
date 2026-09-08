use super::*;

mod control;
mod leases;

pub(in crate::deploy::host_storage_reconcile) use control::*;
pub(in crate::deploy::host_storage_reconcile) use leases::*;

pub(in crate::deploy::host_storage_reconcile) fn parse_queue_control(
    content: Option<&str>,
) -> Result<crate::queue::control::QueueControl, DeployError> {
    match content {
        None => Ok(crate::queue::control::QueueControl::default()),
        Some(content) if content.trim().is_empty() => {
            Ok(crate::queue::control::QueueControl::default())
        }
        Some(content) => serde_json::from_str(content)
            .map_err(|error| DeployError(format!("queue control is invalid: {error}"))),
    }
}

pub(in crate::deploy::host_storage_reconcile) async fn execute_queue_effect(
    store: &crate::queue::JobStorage,
    effect: &QueueEffect,
) -> Result<QueueEffectOutcome, DeployError> {
    let current = store
        .read_text_versioned(crate::queue::control::CONTROL_BLOB)
        .await
        .map_err(|error| DeployError(format!("cannot read queue transition state: {error}")))?;
    let intended = effect.intended.to_json();
    if current
        .as_ref()
        .is_some_and(|versioned| versioned.content == intended)
    {
        return Ok(QueueEffectOutcome::Applied);
    }
    let expected_matches = match (&current, &effect.expected_version, &effect.expected_content) {
        (None, None, None) => true,
        (Some(current), Some(version), Some(content)) => {
            current.version == *version && current.content == *content
        }
        _ => false,
    };
    if !expected_matches {
        return parse_queue_control(current.as_ref().map(|value| value.content.as_str()))
            .map(QueueEffectOutcome::Superseded);
    }
    let write = match current {
        Some(current) => store
            .compare_and_swap_text(
                crate::queue::control::CONTROL_BLOB,
                &current.version,
                &intended,
            )
            .await
            .map(|_| true),
        None => {
            store
                .create_text_if_absent(crate::queue::control::CONTROL_BLOB, &intended)
                .await
        }
    };
    match write {
        Ok(true) => Ok(QueueEffectOutcome::Applied),
        Ok(false)
        | Err(crate::queue::StorageError::StorageConflict(_))
        | Err(crate::queue::StorageError::NotFound(_)) => Err(DeployError(
            "queue control changed during its recorded conditional transition".to_string(),
        )),
        Err(error) => Err(DeployError(format!(
            "cannot apply recorded queue transition: {error}"
        ))),
    }
}
