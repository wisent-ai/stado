//! The measured facts a host verdict is joined from, read once and named.
//!
//! Split out of `join/mod.rs`, which had grown past the module line cap; the
//! truth table that turns these facts into blockers and notes stays there.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::super::payload::diag_flag;
use super::janitor::JanitorHealth;
use crate::deploy::host_disk;
use crate::deploy::host_gates::words::DISK_PRESSURE_UNRESOLVED;
use crate::deploy::host_gates::DISK_PRESSURE_ACTIVE;
use crate::providers::local::host_memory::constants::GIB;
use crate::providers::local::space_reclaim::rule::VolumeReading;
use crate::queue::capacity::Publication;

pub(super) struct Facts {
    pub free_bytes: Option<u64>,
    pub free_gb: Option<f64>,
    pub volume: Option<VolumeReading>,
    pub published_at: Option<String>,
    pub age_seconds: Option<i64>,
    pub stale: bool,
    pub publication_current: bool,
    /// The agent cannot read its volume, so it claims nothing.
    pub disk_pressure_unresolved: bool,
    /// The volume is at the disk-full threshold, published by the agent while
    /// its row is live, measured here otherwise.
    pub disk_full: bool,
    pub pressure_source: Option<&'static str>,
    /// Why this command's own disk reading gave no volume to judge by, when
    /// it gave none: no `df` row, or a field that is not a block count.
    pub local_disk_unread: Option<String>,
    pub cleanup_success_age_seconds: Option<i64>,
    pub janitor: JanitorHealth,
}

impl Facts {
    pub(super) fn read(
        reading: &host_disk::DiskReading,
        publication: Option<&Publication>,
        now: DateTime<Utc>,
        state_observed: bool,
    ) -> Self {
        // This command's own df row for the fleet volume, or why it gave none.
        let local = match reading.usage.as_ref() {
            Some(usage) => host_disk::volume_of(usage),
            None => Err("this command's df read listed no row for the fleet volume".to_string()),
        };
        let (volume, local_disk_unread) = match local {
            Ok(volume) => (Some(volume), None),
            Err(cause) => (None, Some(cause)),
        };
        let free_bytes = volume.map(|volume| volume.free_bytes.unsigned_abs());
        let free_gb = volume.map(|volume| volume.free_bytes as f64 / GIB as f64);

        let payload = publication.map(|row| &row.payload);
        let published_at = payload
            .and_then(|payload| payload.get("published_at"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let age_seconds = publication
            .and_then(|row| row.stamp)
            .map(|stamp| (now - stamp).num_seconds());
        // Past the next publication its author promised, or promising none.
        let stale = publication.is_some_and(|row| row.stale(now));
        let publication_current = publication.is_some() && !stale;

        // The published verdict while the row is live — that IS the decision
        // the agent is making right now. Once the row is stale or absent the
        // agent is no longer talking, so the rule is applied to the numbers
        // this command just measured itself.
        let published_unresolved = diag_flag(payload, DISK_PRESSURE_UNRESOLVED);
        let published_full = diag_flag(payload, DISK_PRESSURE_ACTIVE);
        let disk_pressure_unresolved = publication_current && published_unresolved == Some(true);
        let (disk_full, pressure_source) = match published_full {
            Some(full) if publication_current => (full, Some("capacity_publication")),
            _ => match volume {
                Some(volume) => (volume.full(), Some("host_disk_measurement")),
                None => (false, None),
            },
        };

        // How long since a pass last completed, reported beside the verdict;
        // the verdict itself is the janitor's own promise, read in
        // [`JanitorHealth::read`].
        let cleanup_success_age_seconds = super::janitor::parsed(reading.state.last_success_at.as_deref())
            .map(|stamp| (now - stamp).num_seconds());
        let janitor = JanitorHealth::read(reading, now, state_observed);

        Self {
            free_bytes,
            free_gb,
            volume,
            published_at,
            age_seconds,
            stale,
            publication_current,
            disk_pressure_unresolved,
            disk_full,
            pressure_source,
            local_disk_unread,
            cleanup_success_age_seconds,
            janitor,
        }
    }
}
