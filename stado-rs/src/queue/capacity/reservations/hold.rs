//! Holding a reservation: the acquire write, the heartbeat that keeps it
//! live, and the release that ends it.
//!
//! The host's agent reads a reservation when it publishes its capacity, and
//! it publishes on the period its own publication states. So the holder
//! renews on that same period, read again from the host's publication each
//! round, and each renewal promises the period plus how long its last round
//! took. Neither the renewal interval nor the reservation's lifetime is a
//! number of anyone's choosing.

use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;

use super::{Reservation, ReservationRequest, RESERVATION_SCHEMA_VERSION};
use crate::queue::capacity::{publication_period, CAPACITY_PREFIX};
use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

/// The period at which `consumer_id`'s agent says it publishes, if its
/// publication can be read and states one.
pub async fn consumer_period(store: &JobStorage, consumer_id: &str) -> Option<Duration> {
    let raw = store
        .download_text(&format!("{CAPACITY_PREFIX}{consumer_id}.json"))
        .await
        .ok()??;
    let payload: Value = serde_json::from_str(&raw).ok()?;
    publication_period(&payload)
}

/// Whole seconds covering `promise`, never less than it.
fn seconds_covering(promise: Duration) -> u64 {
    promise.as_secs() + u64::from(promise.subsec_nanos() > 0)
}

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

    /// Rewrite the row with a fresh `heartbeat_at` and the time, `promise`,
    /// within which the holder will renew it again.
    pub async fn heartbeat(
        &mut self,
        store: &JobStorage,
        promise: Duration,
    ) -> Result<(), StorageError> {
        self.reservation.heartbeat_at = Utc::now().to_rfc3339();
        self.reservation.ttl_seconds = seconds_covering(promise);
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
        ttl_seconds: seconds_covering(request.promise),
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

/// Keep `lease` renewed until the returned value is released or dropped:
/// once at once, then every `period` (re-read from the host's publication
/// each round), each renewal promising the period plus the last round. A
/// renewal the store refuses is logged and retried on the next round; the
/// row expires by itself if the store stays gone.
pub fn hold(lease: ReservationLease, store: JobStorage, period: Duration) -> HeldReservation {
    let mut beating = lease.clone();
    let beat_store = store.clone();
    let heartbeat = tokio::spawn(async move {
        let consumer = beating.reservation().consumer_id.clone();
        let mut period = period;
        let mut last_round = Duration::ZERO;
        loop {
            let started = Instant::now();
            if let Some(stated) = consumer_period(&beat_store, &consumer).await {
                period = stated;
            }
            if let Err(error) = beating.heartbeat(&beat_store, period + last_round).await {
                tracing::warn!(
                    reservation = %beating.reservation().reservation_id,
                    "reservation heartbeat was not accepted: {error}"
                );
            }
            last_round = started.elapsed();
            tokio::time::sleep(period).await;
        }
    });
    HeldReservation {
        lease: Some(lease),
        store,
        heartbeat,
    }
}
