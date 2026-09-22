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
use crate::providers::local::disk_cleanup;
use crate::queue::capacity::{self, Publication};
use crate::targets::ComputeTarget;

pub(super) struct Facts {
    pub free_kb: Option<u64>,
    pub free_bytes: Option<u64>,
    pub free_gb: Option<f64>,
    pub low_watermark_gb: Option<i64>,
    pub published_at: Option<String>,
    pub age_seconds: Option<i64>,
    pub stale: bool,
    pub publication_current: bool,
    pub disk_pressure_unresolved: bool,
    pub pressure_source: Option<&'static str>,
    pub cleanup_success_age_seconds: Option<i64>,
    pub janitor: JanitorHealth,
}

impl Facts {
    pub(super) fn read(
        target: &ComputeTarget,
        reading: &host_disk::DiskReading,
        publication: Option<&Publication>,
        now: DateTime<Utc>,
        state_observed: bool,
    ) -> Self {
        let policy = target.disk_cleanup.as_ref();
        let free_kb = reading
            .usage
            .as_ref()
            .and_then(|usage| usage.available_kb.parse::<u64>().ok());
        let free_bytes = free_kb.and_then(|blocks| blocks.checked_mul(1024));
        let free_gb = free_kb.map(|blocks| host_disk::gib_from_blocks(blocks as f64));
        // The registry's declared watermark first, and the janitor's state file
        // only where the registry declares no policy at all.
        //
        // This was the other way round, on the reasoning that the state file holds
        // the number the agent actually gated on and survives a registry the host
        // cannot read. Both halves are true and it still reported a number that
        // could not be acted on. That file is written by every cleanup pass, and on
        // an always-on host several processes make them: the queue agent every ten
        // seconds, a `disk-cleanup --watch` unit on its own timer, and any of them
        // may be a long-running process still holding a configuration that resolves
        // a superseded policy. On charless-mac-mini that produced `low watermark
        // 20 GiB, target 18 GiB` — a floor above its own ceiling, from a stale
        // 20/25 policy — alternating with the canonical 15/18 between one reading
        // and the next, while the registry said 15 throughout.
        //
        // So the declaration wins. It is what the fleet decided, this command has
        // just read it, and a watermark the operator cannot reconcile with the
        // policy document is worse than no watermark at all.
        let low_watermark_gb = policy.map(|policy| policy.low_free_gb).or_else(|| {
            reading
                .state
                .low_bytes
                .map(|bytes| bytes / disk_cleanup::GIB)
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

        // The published verdict while the row is live — that IS the decision the
        // agent is making right now. Once the row is stale or absent the agent is
        // no longer talking, so the same function it uses is applied to the
        // numbers this command just measured itself.
        let published_pressure = diag_flag(payload, DISK_PRESSURE_UNRESOLVED);
        let disk_pressure_unresolved = match published_pressure {
            Some(published) if publication_current => published,
            // Scheduler conservatism is not a measured disk-pressure diagnosis.
            _ if free_bytes.is_none() || low_watermark_gb.is_none() => false,
            _ => disk_cleanup::disk_pressure_unresolved(
                low_watermark_gb.map(|gb| gb * disk_cleanup::GIB),
                free_bytes.and_then(|bytes| i64::try_from(bytes).ok()),
            ),
        };
        let pressure_source = match published_pressure {
            Some(_) if publication_current => Some("capacity_publication"),
            _ if low_watermark_gb.is_some() && free_kb.is_some() => Some("host_disk_measurement"),
            _ => None,
        };

        // How late the janitor is against the interval IT declares, measured from
        // the last pass that actually completed. `last_success_at` and not
        // `last_pass_at`: the incident this exists for logged a pass every sixty
        // seconds for fifteen days, so "it ran recently" was true throughout and
        // meant nothing.
        let cleanup_success_age_seconds = reading
            .state
            .last_success_at
            .as_deref()
            .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
            .map(|stamp| (now - stamp.with_timezone(&Utc)).num_seconds());
        // Armed only where the host declares a janitor that is supposed to run.
        // `mode: "off"` is a deliberate choice and never late, and a host with no
        // declared interval has nothing to be late against — that is
        // `disk_cleanup_policy_unknown`, which is already a blocker of its own.
        let stall_after_seconds = policy
            .filter(|policy| policy.mode != "off")
            .map(|policy| policy.check_interval_seconds * STALL_INTERVALS);
        let janitor = JanitorHealth::read(
            reading,
            now,
            state_observed,
            stall_after_seconds,
            cleanup_success_age_seconds,
        );

        Self {
            free_kb,
            free_bytes,
            free_gb,
            low_watermark_gb,
            published_at,
            age_seconds,
            stale,
            publication_current,
            disk_pressure_unresolved,
            pressure_source,
            cleanup_success_age_seconds,
            janitor,
        }
    }
}
