//! Claiming `machine_requests/<id>.json`: replay an identical completed
//! request, refuse a conflicting one, otherwise take the lease.

use serde_json::{Map, Value};

use crate::machine::contract::encoding::{canonical_json, request_digest, utcnow};
use crate::machine::contract::SCHEMA_VERSION;
use crate::machine::sources::staging::stage_source_archive;
use crate::machine::sources::{OwnedMachineFile, MAX_SOURCE_ARCHIVE_BYTES};
use crate::machine::{MachineError, MachineFacade};
use crate::queue::StorageError;

use super::{ClaimedReservation, ReservationClaim, SubmitRequestContext};

impl MachineFacade {
    /// Reserve the request record under a fresh lease, or hand back the
    /// result an exact earlier retry already stored.
    pub(super) async fn claim_request_reservation(
        &self,
        ctx: &SubmitRequestContext,
    ) -> Result<ReservationClaim, MachineError> {
        let SubmitRequestContext {
            request,
            request_id,
            record_path,
            run_id,
            owner,
            lease_expires_at,
            ..
        } = ctx;
        let source_requested = ctx.source_requested;
        let mut source_uri = String::new();
        let mut source_sha = String::new();
        let mut source_bytes = 0u64;
        let mut staged_source: Option<OwnedMachineFile> = None;
        let mut replayed_reservation = false;
        let mut claimed = false;

        for _ in 0..16 {
            if let Some(staged) = staged_source.take() {
                staged.cleanup().map_err(|error| {
                    MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string())
                })?;
            }
            if let Some(versioned) = self.store.read_text_versioned(record_path).await? {
                let mut existing: Map<String, Value> =
                    serde_json::from_str::<Value>(&versioned.content)
                        .map_err(|_| {
                            MachineError::new("INTERNAL", "stored idempotency record is invalid")
                        })?
                        .as_object()
                        .cloned()
                        .ok_or_else(|| {
                            MachineError::new("INTERNAL", "stored idempotency record is invalid")
                        })?;
                let retained_sha = existing
                    .get("source_sha256")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let retained_uri = existing
                    .get("source_archive_uri")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let retained_bytes = existing
                    .get("source_size_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                if source_requested != !retained_sha.is_empty()
                    || retained_sha.is_empty() != retained_uri.is_empty()
                    || source_requested != (retained_bytes != 0)
                {
                    return Err(MachineError::new(
                        "IDEMPOTENCY_CONFLICT",
                        "client_request_id was already used with a different request",
                    ));
                }
                if source_requested {
                    if retained_sha.len() != 64
                        || !retained_sha
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    {
                        return Err(MachineError::new(
                            "INTERNAL",
                            "stored source digest is invalid",
                        ));
                    }
                    if retained_bytes > MAX_SOURCE_ARCHIVE_BYTES {
                        return Err(MachineError::new(
                            "INTERNAL",
                            "stored source byte count is invalid",
                        ));
                    }
                    let expected_source = crate::object_store::ObjectRef::new(
                        "machine-inputs",
                        &format!("{request_id}/{retained_sha}.tar.gz"),
                    )?;
                    if retained_uri != expected_source.to_string() {
                        return Err(MachineError::new(
                            "INTERNAL",
                            "stored source object identity is invalid",
                        ));
                    }
                }
                if source_requested {
                    let (staged, current_sha, current_bytes) = stage_source_archive(
                        request.get("source_archive_path"),
                    )?
                    .ok_or_else(|| {
                        MachineError::new(
                            "INVALID_SOURCE_ARCHIVE",
                            "source archive path is required",
                        )
                    })?;
                    if current_sha != retained_sha || current_bytes != retained_bytes {
                        return Err(MachineError::new(
                            "IDEMPOTENCY_CONFLICT",
                            "client_request_id was already used with a different source archive",
                        ));
                    }
                    staged_source = Some(staged);
                }
                let mut digest_request = request.clone();
                if source_requested {
                    digest_request.insert("source_archive_path".into(), Value::from(retained_sha));
                }
                let digest = request_digest(&digest_request);
                if existing.get("request_digest").and_then(Value::as_str) != Some(digest.as_str())
                    || existing.get("run_id").and_then(Value::as_str) != Some(run_id.as_str())
                {
                    return Err(MachineError::new(
                        "IDEMPOTENCY_CONFLICT",
                        "client_request_id was already used with a different request",
                    ));
                }
                if let Some(stored_result) =
                    existing.get("result").filter(|result| result.is_object())
                {
                    if stored_result.get("job").is_some_and(Value::is_object) {
                        return Ok(ReservationClaim::Replayed(stored_result.clone()));
                    }
                }
                let live_lease = existing
                    .get("lease_expires_at")
                    .and_then(Value::as_str)
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .is_some_and(|expires| expires > chrono::Utc::now());
                if live_lease {
                    return Err(MachineError::retryable(
                        "REQUEST_IN_PROGRESS",
                        "matching request is still being submitted",
                    ));
                }
                source_sha = retained_sha.to_string();
                source_uri = retained_uri.to_string();
                source_bytes = retained_bytes;
                existing.insert("state".into(), Value::from("claimed"));
                existing.insert("owner".into(), Value::from(owner.as_str()));
                existing.insert(
                    "lease_expires_at".into(),
                    Value::from(lease_expires_at.as_str()),
                );
                match self
                    .store
                    .compare_and_swap_text(
                        record_path,
                        &versioned.version,
                        &canonical_json(&Value::Object(existing)),
                    )
                    .await
                {
                    Ok(_) => {
                        claimed = true;
                        replayed_reservation = true;
                        break;
                    }
                    Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                    Err(error) => return Err(error.into()),
                }
            }

            let mut digest_request = request.clone();
            if source_requested {
                let (staged, sha, bytes) =
                    stage_source_archive(request.get("source_archive_path"))?.ok_or_else(|| {
                        MachineError::new(
                            "INVALID_SOURCE_ARCHIVE",
                            "source archive path is required",
                        )
                    })?;
                staged_source = Some(staged);
                source_sha = sha;
                source_bytes = bytes;
                let source_object = crate::object_store::ObjectRef::new(
                    "machine-inputs",
                    &format!("{request_id}/{source_sha}.tar.gz"),
                )?;
                source_uri = source_object.to_string();
                digest_request.insert(
                    "source_archive_path".into(),
                    Value::from(source_sha.as_str()),
                );
            }
            let digest = request_digest(&digest_request);
            let mut reservation = Map::new();
            reservation.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
            reservation.insert("client_request_id".into(), Value::from(request_id.as_str()));
            reservation.insert("request_digest".into(), Value::from(digest));
            reservation.insert("run_id".into(), Value::from(run_id.as_str()));
            reservation.insert("state".into(), Value::from("claimed"));
            reservation.insert("owner".into(), Value::from(owner.as_str()));
            reservation.insert(
                "lease_expires_at".into(),
                Value::from(lease_expires_at.as_str()),
            );
            reservation.insert("created_at".into(), Value::from(utcnow()));
            if source_requested {
                reservation.insert(
                    "source_archive_uri".into(),
                    Value::from(source_uri.as_str()),
                );
                reservation.insert("source_sha256".into(), Value::from(source_sha.as_str()));
                reservation.insert("source_size_bytes".into(), Value::from(source_bytes));
            }
            if self
                .store
                .create_text_if_absent(record_path, &canonical_json(&Value::Object(reservation)))
                .await?
            {
                claimed = true;
                break;
            }
        }
        if !claimed {
            return Err(MachineError::retryable(
                "REQUEST_IN_PROGRESS",
                "request reservation remained contended",
            ));
        }
        Ok(ReservationClaim::Claimed(ClaimedReservation {
            source_uri,
            source_sha,
            source_bytes,
            staged_source,
            replayed_reservation,
        }))
    }
}
