//! Holding a reservation: the acquire write, the heartbeat that keeps it
//! live, and the release that ends it.

use std::time::Duration;

use chrono::Utc;

use super::{Reservation, ReservationRequest, RESERVATION_SCHEMA_VERSION};
use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

/// One acquired reservation: heartbeat it while the work runs, release it
/// when the work ends.
#[derive(Debug, Clone)]
pub struct ReservationLease {
    reservation: Reservation,
}

impl ReservationLease {
    pub fn reservation(&self) -> &Reservation {
        &self.reservation
    }

    /// Rewrite the row with a fresh `heartbeat_at`.
    pub async fn heartbeat(&mut self, store: &JobStorage) -> Result<(), StorageError> {
        self.reservation.heartbeat_at = Utc::now().to_rfc3339();
        let body = serde_json::to_string(&self.reservation)?;
        store.upload_text(&self.reservation.key(), &body).await
    }

    /// Delete the row. Idempotent: a row already gone is a released row.
    pub async fn release(self, store: &JobStorage) -> Result<(), StorageError> {
        store.delete_blob(&self.reservation.key()).await
    }
}

/// Create the reservation. The id is fresh, so the create-if-absent write
/// has exactly one winner and a retried caller never doubles a hold.
pub async fn acquire(
    store: &JobStorage,
    request: ReservationRequest,
) -> Result<ReservationLease, StorageError> {
    let now = Utc::now().to_rfc3339();
    let reservation = Reservation {
        schema_version: RESERVATION_SCHEMA_VERSION,
        reservation_id: uuid::Uuid::new_v4().to_string(),
        consumer_id: request.consumer_id,
        target: request.target,
        kind: request.kind,
        product: request.product,
        holder: request.holder,
        cpu_cores: request.cpu_cores,
        ram_gb: request.ram_gb,
        vram_gb: request.vram_gb,
        acquired_at: now.clone(),
        heartbeat_at: now,
        ttl_seconds: request.ttl_seconds,
    };
    let body = serde_json::to_string(&reservation)?;
    if !store
        .create_text_if_absent(&reservation.key(), &body)
        .await?
    {
        return Err(StorageError::StorageConflict(format!(
            "reservation {} already exists",
            reservation.reservation_id
        )));
    }
    Ok(ReservationLease { reservation })
}

/// A reservation kept alive by a background heartbeat for as long as this
/// value lives. Dropping it stops the heartbeat and lets the row expire; a
/// clean end calls [`HeldReservation::release`] so the row is gone at once.
pub struct HeldReservation {
    lease: Option<ReservationLease>,
    store: JobStorage,
    heartbeat: tokio::task::JoinHandle<()>,
}

impl HeldReservation {
    pub fn reservation(&self) -> Option<&Reservation> {
        self.lease.as_ref().map(ReservationLease::reservation)
    }

    /// Stop the heartbeat and delete the row.
    pub async fn release(mut self) -> Result<(), StorageError> {
        self.heartbeat.abort();
        match self.lease.take() {
            Some(lease) => lease.release(&self.store).await,
            None => Ok(()),
        }
    }
}

impl Drop for HeldReservation {
    fn drop(&mut self) {
        self.heartbeat.abort();
    }
}

/// Keep `lease` heartbeated every `interval` until the returned value is
/// released or dropped. A heartbeat the store refuses is logged and retried
/// on the next interval; the row expires by itself if the store stays gone.
pub fn hold(lease: ReservationLease, store: JobStorage, interval: Duration) -> HeldReservation {
    let mut beating = lease.clone();
    let beat_store = store.clone();
    let heartbeat = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = beating.heartbeat(&beat_store).await {
                tracing::warn!(
                    reservation = %beating.reservation().reservation_id,
                    "reservation heartbeat was not accepted: {error}"
                );
            }
        }
    });
    HeldReservation {
        lease: Some(lease),
        store,
        heartbeat,
    }
}
