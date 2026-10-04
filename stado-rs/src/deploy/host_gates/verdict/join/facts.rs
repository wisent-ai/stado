//! The measured facts a host verdict is joined from, read once and named.
//!
//! Split out of `join/mod.rs`, which had grown past the module line cap; the
//! truth table that turns these facts into blockers and notes stays there.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::super::payload::diag_flag;
use super::janitor::JanitorHealth;
use crate::deploy::host_disk;
use crate::deploy::host_gates::words::{DISK_PRESSURE_UNRESOLVED, STALL_INTERVALS};
use crate::deploy::host_gates::DISK_PRESSURE_ACTIVE;
use crate::providers::local::disk_cleanup::rule::{self, VolumeReading};
use crate::queue::capacity::{self, Publication};

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
        let kib = |value: &str| value.parse::<u64>().ok();
        let free_kb = reading
            .usage
            .as_ref()
            .and_then(|usage| kib(&usage.available_kb));
        let free_bytes = free_kb.and_then(|blocks| blocks.checked_mul(1024));
        let free_gb = free_kb.map(|blocks| host_disk::gib_from_blocks(blocks as f64));
        let volume = reading.usage.as_ref().and_then(|usage| {
            Some(VolumeReading {
                total_bytes: i64::try_from(kib(&usage.blocks_kb)?.checked_mul(1024)?).ok()?,
                free_bytes: i64::try_from(kib(&usage.available_kb)?.checked_mul(1024)?).ok()?,
            })
        });

        let payload = publication.map(|row| &row.payload);
        let published_at = payload
            .and_then(|payload| payload.get("published_at"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let age_seconds = publication
            .and_then(|row| row.stamp)
            .map(|stamp| (now - stamp).num_seconds());
        let stale = age_seconds.is_some_and(|age| age > capacity::CAPACITY_STALE_SECONDS as i64);
        let publication_current = age_seconds.is_some() && !stale;

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

        // Lateness is measured from the last successful pass against the
        // rule's check cadence. A recent attempt is not evidence that cleanup
        // completed, so last_pass_at cannot establish freshness.
        let cleanup_success_age_seconds = reading
            .state
            .last_success_at
            .as_deref()
            .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
            .map(|stamp| (now - stamp.with_timezone(&Utc)).num_seconds());
        let stall_after_seconds = i64::try_from(rule::CHECK_SECONDS)
            .unwrap_or(i64::MAX)
            .saturating_mul(STALL_INTERVALS);
        let janitor = JanitorHealth::read(
            reading,
            now,
            state_observed,
            Some(stall_after_seconds),
            cleanup_success_age_seconds,
        );

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
            cleanup_success_age_seconds,
            janitor,
        }
    }
}
