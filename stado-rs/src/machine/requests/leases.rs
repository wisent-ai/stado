//! Renewing the reservation lease so a long submission is never mistaken for
//! an abandoned one.

use serde_json::{Map, Value};

use crate::machine::contract::encoding::canonical_json;
use crate::machine::MachineError;
use crate::queue::{JobStorage, StorageError};

async fn renew_machine_request_lease(
    store: &JobStorage,
    record_path: &str,
    owner: &str,
    expected_state: &str,
    phase: &str,
) -> Result<(), MachineError> {
    for _ in 0..16 {
        let versioned = store
            .read_text_versioned(record_path)
            .await?
            .ok_or_else(|| {
                MachineError::retryable("REQUEST_IN_PROGRESS", "reservation disappeared")
            })?;
        let mut reservation: Map<String, Value> =
            serde_json::from_str::<Value>(&versioned.content)?
                .as_object()
                .cloned()
                .ok_or_else(|| {
                    MachineError::new("INTERNAL", "stored idempotency record is invalid")
                })?;
        if reservation.get("owner").and_then(Value::as_str) != Some(owner)
            || reservation.get("state").and_then(Value::as_str) != Some(expected_state)
        {
            return Err(MachineError::retryable(
                "REQUEST_IN_PROGRESS",
                "matching request ownership changed during submission",
            ));
        }
        reservation.insert(
            "lease_expires_at".into(),
            Value::from((chrono::Utc::now() + chrono::Duration::minutes(15)).to_rfc3339()),
        );
        reservation.insert("phase".into(), Value::from(phase));
        match store
            .compare_and_swap_text(
                record_path,
                &versioned.version,
                &canonical_json(&Value::Object(reservation)),
            )
            .await
        {
            Ok(_) => return Ok(()),
            Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(MachineError::retryable(
        "REQUEST_IN_PROGRESS",
        "request lease renewal remained contended",
    ))
}

pub(in crate::machine) async fn renew_machine_request_claim(
    store: &JobStorage,
    record_path: &str,
    owner: &str,
    phase: &str,
) -> Result<(), MachineError> {
    renew_machine_request_lease(store, record_path, owner, "claimed", phase).await
}

pub(in crate::machine) async fn renew_machine_request_enqueue(
    store: &JobStorage,
    record_path: &str,
    owner: &str,
    phase: &str,
) -> Result<(), MachineError> {
    renew_machine_request_lease(store, record_path, owner, "enqueuing", phase).await
}
