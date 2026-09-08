//! The idempotent submission protocol, one phase per component: claim the
//! reservation, materialize the source, enqueue, then accept.

use serde_json::{Map, Value};

use crate::machine::requests::validate::validate_request;
use crate::machine::sources::OwnedMachineFile;
use crate::machine::{MachineError, MachineFacade};
use crate::queue::submit::stable_run_id;

mod acceptance;
mod enqueue;
mod reservation;
mod source;

/// The validated request and the reservation identity every submission phase
/// works against. Each phase destructures only the fields it reads.
struct SubmitRequestContext {
    request: Map<String, Value>,
    request_id: String,
    source_requested: bool,
    record_path: String,
    run_id: String,
    owner: String,
    lease_expires_at: String,
}

/// What the reservation record says about the source archive, once this call
/// holds the lease on it.
struct ClaimedReservation {
    source_uri: String,
    source_sha: String,
    source_bytes: u64,
    staged_source: Option<OwnedMachineFile>,
    replayed_reservation: bool,
}

/// Either the result an identical earlier request already stored, or the
/// reservation this call now owns.
enum ReservationClaim {
    Replayed(Value),
    Claimed(ClaimedReservation),
}

impl MachineFacade {
    /// Idempotent submit (Python `submit_request`): validate, reserve
    /// `machine_requests/<id>.json` with the SHA-256 request digest, replay
    /// stored results on exact retry, reject digest mismatches with
    /// IDEMPOTENCY_CONFLICT.
    pub async fn submit_request(&self, request: &Value) -> Result<Value, MachineError> {
        let request = validate_request(request)?;
        let request_id = request["client_request_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let source_requested = request["source_archive_path"]
            .as_str()
            .is_some_and(|path| !path.is_empty());
        let record_path = format!("machine_requests/{request_id}.json");
        let run_id = stable_run_id("machine", &request_id);
        let owner = uuid::Uuid::new_v4().simple().to_string();
        let lease_expires_at = (chrono::Utc::now() + chrono::Duration::minutes(15)).to_rfc3339();
        let ctx = SubmitRequestContext {
            request,
            request_id,
            source_requested,
            record_path,
            run_id,
            owner,
            lease_expires_at,
        };

        let mut reserved = match self.claim_request_reservation(&ctx).await? {
            ReservationClaim::Replayed(result) => return Ok(result),
            ReservationClaim::Claimed(reservation) => reservation,
        };
        self.upload_machine_source(&ctx, &mut reserved).await?;
        let job = self.enqueue_machine_request(&ctx, &reserved).await?;
        self.accept_machine_request(&ctx, &reserved, &job).await
    }
}
