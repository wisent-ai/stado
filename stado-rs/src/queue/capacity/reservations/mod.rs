//! Capacity reservations: what a placed workload holds on a host while it
//! runs, so the host's capacity publication can subtract it.
//!
//! The queue's own jobs are counted by the agent that runs them. Everything
//! else Stado places on a host — an interactive Jeden session over
//! `stado workload attach`, a browser task over `stado workload run` — holds
//! nothing unless it is written down: the host keeps publishing itself as
//! free and can be handed any number of Jeden sessions while the scheduler
//! reads it as idle. A reservation is that object: one
//! document per placed workload, renewed while the process lives, subtracted
//! by the agent from what it publishes, and gone when the process ends or
//! its holder stops renewing.
//!
//! Scheme: `state/reservations/<consumer_id>/<reservation_id>.json`, a
//! [`Reservation`] serialized as written. A reservation is live while
//! `heartbeat_at + ttl_seconds` is in the future: `ttl_seconds` is the
//! holder's own promise, the host's publication period plus how long the
//! holder's last renewal round took. It lives under `state/` because that is
//! the root the object gateway authorizes for fleet state; `autonomy/` and
//! `capacity/…` would be refused with the 401
//! [`crate::autonomy::storage::OBJECT_ROOT`] documents.
//!
//! `hold` carries the lease: acquiring, renewing and releasing one.

mod hold;

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::StorageError;
use crate::primitives::constants;
use crate::queue::storage::JobStorage;

pub use hold::{acquire, consumer_period, hold, HeldReservation, ReservationLease};

pub const RESERVATION_PREFIX: &str = "state/reservations/";
pub const RESERVATION_SCHEMA_VERSION: u64 = constants::RESERVATION_SCHEMA_VERSION;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reservation {
    pub schema_version: u64,
    pub reservation_id: String,
    pub consumer_id: String,
    pub target: String,
    pub kind: String,
    pub product: String,
    pub holder: String,
    pub cpu_cores: i64,
    pub ram_gb: f64,
    pub vram_gb: i64,
    pub acquired_at: String,
    pub heartbeat_at: String,
    pub ttl_seconds: u64,
}

impl Reservation {
    pub fn key(&self) -> String {
        reservation_key(&self.consumer_id, &self.reservation_id)
    }

    fn heartbeat(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.heartbeat_at)
            .ok()
            .map(|stamp| stamp.with_timezone(&Utc))
    }

    /// The instant this reservation stops counting unless heartbeated.
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.heartbeat()
            .map(|beat| beat + chrono::Duration::seconds(self.ttl_seconds as i64))
    }

    /// An undateable row is not live: a reservation that cannot say when it
    /// was last heartbeated cannot claim capacity.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.expires_at().is_some_and(|expiry| expiry > now)
    }

    /// The compact row the capacity publication carries.
    pub fn published(&self) -> Value {
        serde_json::json!({
            "reservation_id": self.reservation_id,
            "kind": self.kind,
            "product": self.product,
            "holder": self.holder,
            "cpu_cores": self.cpu_cores,
            "ram_gb": self.ram_gb,
            "vram_gb": self.vram_gb,
            "acquired_at": self.acquired_at,
        })
    }
}

pub fn reservation_key(consumer_id: &str, reservation_id: &str) -> String {
    format!("{RESERVATION_PREFIX}{consumer_id}/{reservation_id}.json")
}

/// What a caller asks to hold. Sizes come from the workload declaration,
/// never from the caller's guess.
#[derive(Debug, Clone, PartialEq)]
pub struct ReservationRequest {
    pub consumer_id: String,
    pub target: String,
    pub kind: String,
    pub product: String,
    pub holder: String,
    pub cpu_cores: i64,
    pub ram_gb: f64,
    pub vram_gb: i64,
    /// The time within which the holder will first renew: the host's
    /// publication period.
    pub promise: Duration,
}

/// The sum of a set of reservations, in the units the publication uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Reserved {
    pub cpu_cores: i64,
    pub ram_gb: f64,
    pub vram_gb: i64,
}

impl Reserved {
    pub fn of(reservations: &[Reservation]) -> Self {
        reservations.iter().fold(Self::default(), |sum, row| Self {
            cpu_cores: sum.cpu_cores + row.cpu_cores,
            ram_gb: sum.ram_gb + row.ram_gb,
            vram_gb: sum.vram_gb + row.vram_gb,
        })
    }
}

/// Every reservation row, live or not, grouped by consumer. A report's
/// reader: it deletes nothing and skips only a row that vanished between the
/// listing and the download.
pub async fn read_all(
    store: &JobStorage,
) -> Result<BTreeMap<String, Vec<Reservation>>, StorageError> {
    let mut rows: BTreeMap<String, Vec<Reservation>> = BTreeMap::new();
    for blob in store.list_blobs_with_meta(RESERVATION_PREFIX).await? {
        if !blob.name.ends_with(".json") {
            continue;
        }
        let Some(raw) = store.download_text(&blob.name).await? else {
            continue;
        };
        let reservation: Reservation = serde_json::from_str(&raw)?;
        rows.entry(reservation.consumer_id.clone())
            .or_default()
            .push(reservation);
    }
    for list in rows.values_mut() {
        list.sort_by(|left, right| left.acquired_at.cmp(&right.acquired_at));
    }
    Ok(rows)
}

/// The live reservations one consumer must subtract from what it publishes.
pub async fn live_for_consumer(
    store: &JobStorage,
    consumer_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Reservation>, StorageError> {
    Ok(sweep_for_consumer(store, consumer_id, now).await?.live)
}

/// One consumer's rows after a sweep: the live ones, and how many rows past
/// their holder's promise were retired on the way.
#[derive(Debug, Default)]
pub struct ConsumerSweep {
    pub live: Vec<Reservation>,
    pub retired: usize,
}

/// Read one consumer's reservations and, in the same pass, delete every row
/// whose holder's promise has passed: its holder stopped renewing, and the
/// row holds nothing. One listing serves both, so the agent's publish path
/// pays no second round trip for its own hygiene.
pub async fn sweep_for_consumer(
    store: &JobStorage,
    consumer_id: &str,
    now: DateTime<Utc>,
) -> Result<ConsumerSweep, StorageError> {
    let directory = format!("{RESERVATION_PREFIX}{consumer_id}/");
    let mut sweep = ConsumerSweep::default();
    for blob in store.list_blobs_with_meta(&directory).await? {
        if !blob.name.starts_with(&directory) || !blob.name.ends_with(".json") {
            continue;
        }
        let Some(raw) = store.download_text(&blob.name).await? else {
            continue;
        };
        let reservation: Reservation = serde_json::from_str(&raw)?;
        if reservation.is_live(now) {
            sweep.live.push(reservation);
        } else {
            store.delete_blob(&blob.name).await?;
            sweep.retired += 1;
        }
    }
    sweep
        .live
        .sort_by(|left, right| left.acquired_at.cmp(&right.acquired_at));
    Ok(sweep)
}
