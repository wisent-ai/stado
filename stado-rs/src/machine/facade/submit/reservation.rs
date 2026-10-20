//! Claiming `machine_requests/<id>.json`: replay an identical completed
//! request, refuse a conflicting one, otherwise take the lease.

use serde_json::{Map, Value};

use crate::machine::contract::encoding::{canonical_json, request_digest, utcnow};
use crate::machine::contract::SCHEMA_VERSION;
use crate::machine::sources::staging::stage_source_archive;
use crate::machine::sources::OwnedMachineFile;
use crate::machine::{MachineError, MachineFacade};
use crate::queue::StorageError;

use super::{ClaimedReservation, ReservationClaim, SubmitRequestContext};

fn invalid(field: &str, found: &Value) -> MachineError {
    MachineError::new(
        "INTERNAL",
        format!("stored idempotency record field {field} is not what it should be: {found}"),
    )
}

fn retained_text(record: &Map<String, Value>, field: &str) -> Result<Option<String>, MachineError> {
    match record.get(field) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(other) => Err(invalid(field, other)),
    }
}

fn retained_size(record: &Map<String, Value>) -> Result<Option<u64>, MachineError> {
    match record.get("source_size_bytes") {
        None => Ok(None),
        Some(found) => match found.as_u64() {
            Some(bytes) => Ok(Some(bytes)),
            None => Err(invalid("source_size_bytes", found)),
        },
    }
}

/// The request's own conflict: the record names another request.
fn conflict(detail: &str) -> MachineError {
    MachineError::new("IDEMPOTENCY_CONFLICT", detail.to_string())
}

impl MachineFacade {
    /// Reserve the request record under a fresh lease, or hand back the
    /// result an exact earlier retry already stored.
    ///
    /// Each pass reads the record and ends on what it found: a replay of the
    /// stored result, a conflicting request, a reservation another submission
    /// holds, or this submission's own reservation taken by compare-and-swap
    /// or created. A swap or a create that another writer won is no answer;
    /// the next pass reads what that writer left, so the loop stops on the
    /// record's state, never after a count of tries.
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
            ..
        } = ctx;
        let source_requested = ctx.source_limits.is_some();
        let mut staged_source: Option<OwnedMachineFile> = None;

        loop {
            if let Some(staged) = staged_source.take() {
                staged.cleanup().map_err(|error| {
                    MachineError::retryable("SOURCE_UPLOAD_FAILED", error.to_string())
                })?;
            }
            if let Some(versioned) = self.store.read_text_versioned(record_path).await? {
                let stored =
                    serde_json::from_str::<Value>(&versioned.content).map_err(|error| {
                        MachineError::new(
                            "INTERNAL",
                            format!("stored idempotency record is not JSON: {error}"),
                        )
                    })?;
                let mut existing: Map<String, Value> = match stored {
                    Value::Object(existing) => existing,
                    other => {
                        return Err(MachineError::new(
                            "INTERNAL",
                            format!("stored idempotency record is not an object: {other}"),
                        ))
                    }
                };
                let retained_sha = retained_text(&existing, "source_sha256")?;
                let retained_uri = retained_text(&existing, "source_archive_uri")?;
                let retained_bytes = retained_size(&existing)?;
                // A record holds all three source fields or none, and holds
                // them exactly when this request carries an archive.
                let (source_sha, source_uri) = match (&retained_sha, &retained_uri, retained_bytes)
                {
                    (Some(sha), Some(uri), Some(_)) if source_requested => {
                        (sha.clone(), uri.clone())
                    }
                    (None, None, None) if !source_requested => (String::new(), String::new()),
                    _ => {
                        return Err(conflict(
                            "client_request_id was already used with a different request",
                        ))
                    }
                };
                if let (Some(limits), Some(bytes)) = (&ctx.source_limits, retained_bytes) {
                    if !crate::deploy::host_release::is_sha256(&source_sha) {
                        return Err(MachineError::new(
                            "INTERNAL",
                            format!("stored source digest {source_sha:?} is not a SHA-256"),
                        ));
                    }
                    if bytes > limits.archive_bytes.get() {
                        return Err(MachineError::new(
                            "INVALID_SOURCE_ARCHIVE",
                            format!(
                                "retained source archive has {bytes} bytes; machine.source_limits.archive_bytes permits {}",
                                limits.archive_bytes
                            ),
                        ));
                    }
                    let expected_source = crate::remote::object_store::ObjectRef::new(
                        "machine-inputs",
                        &format!("{request_id}/{source_sha}.tar.gz"),
                    )?;
                    if source_uri != expected_source.to_string() {
                        return Err(MachineError::new(
                            "INTERNAL",
                            "stored source object identity is invalid",
                        ));
                    }
                    let (staged, current_sha, current_bytes) =
                        stage_source_archive(request.get("source_archive_path"), limits)?
                            .ok_or_else(|| {
                                MachineError::new(
                                    "INVALID_SOURCE_ARCHIVE",
                                    "source archive path is required",
                                )
                            })?;
                    if current_sha != source_sha || current_bytes != bytes {
                        return Err(conflict(
                            "client_request_id was already used with a different source archive",
                        ));
                    }
                    staged_source = Some(staged);
                }
                let mut digest_request = request.clone();
                if source_requested {
                    digest_request.insert(
                        "source_archive_path".into(),
                        Value::from(source_sha.as_str()),
                    );
                }
                let digest = request_digest(&digest_request);
                if existing.get("request_digest").and_then(Value::as_str) != Some(digest.as_str())
                    || existing.get("run_id").and_then(Value::as_str) != Some(run_id.as_str())
                {
                    return Err(conflict(
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
                // A reservation another submission holds stays held until
                // that submission records its result or releases it.
                if matches!(
                    existing.get("state").and_then(Value::as_str),
                    Some("claimed" | "enqueuing")
                ) {
                    return Err(MachineError::retryable(
                        "REQUEST_IN_PROGRESS",
                        "matching request is held by a submission that has not released it",
                    ));
                }
                existing.insert("state".into(), Value::from("claimed"));
                existing.insert("owner".into(), Value::from(owner.as_str()));
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
                        return Ok(ReservationClaim::Claimed(ClaimedReservation {
                            source_uri,
                            source_sha,
                            source_bytes: retained_bytes,
                            staged_source,
                            replayed_reservation: true,
                        }))
                    }
                    // Another writer changed or removed the record since this
                    // pass read it; the next pass reads what it left.
                    Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => continue,
                    Err(error) => return Err(error.into()),
                }
            }

            let mut digest_request = request.clone();
            let mut reservation = Map::new();
            let (source_uri, source_sha, source_bytes) = match &ctx.source_limits {
                Some(limits) => {
                    let (staged, sha, bytes) =
                        stage_source_archive(request.get("source_archive_path"), limits)?
                            .ok_or_else(|| {
                                MachineError::new(
                                    "INVALID_SOURCE_ARCHIVE",
                                    "source archive path is required",
                                )
                            })?;
                    staged_source = Some(staged);
                    let source_object = crate::remote::object_store::ObjectRef::new(
                        "machine-inputs",
                        &format!("{request_id}/{sha}.tar.gz"),
                    )?;
                    let uri = source_object.to_string();
                    digest_request.insert("source_archive_path".into(), Value::from(sha.as_str()));
                    reservation.insert("source_archive_uri".into(), Value::from(uri.as_str()));
                    reservation.insert("source_sha256".into(), Value::from(sha.as_str()));
                    reservation.insert("source_size_bytes".into(), Value::from(bytes));
                    (uri, sha, Some(bytes))
                }
                // A request without a source archive keeps no source fields.
                None => (String::new(), String::new(), None),
            };
            reservation.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
            reservation.insert("client_request_id".into(), Value::from(request_id.as_str()));
            reservation.insert(
                "request_digest".into(),
                Value::from(request_digest(&digest_request)),
            );
            reservation.insert("run_id".into(), Value::from(run_id.as_str()));
            reservation.insert("state".into(), Value::from("claimed"));
            reservation.insert("owner".into(), Value::from(owner.as_str()));
            reservation.insert("created_at".into(), Value::from(utcnow()));
            if self
                .store
                .create_text_if_absent(record_path, &canonical_json(&Value::Object(reservation)))
                .await?
            {
                return Ok(ReservationClaim::Claimed(ClaimedReservation {
                    source_uri,
                    source_sha,
                    source_bytes,
                    staged_source,
                    replayed_reservation: false,
                }));
            }
            // Another submission created the record first; the next pass
            // reads it and replays, refuses or reports it held.
        }
    }
}
