//! The clock every archived document is stamped from: the lease window a lock
//! is taken for, the wire timestamp a record carries, and the compact stamp an
//! event file is named by.

use chrono::{DateTime, Duration, SecondsFormat, Utc};

pub(super) fn lease_duration() -> Duration {
    Duration::hours((true as i64).saturating_add(true as i64))
}

pub(super) fn now() -> String {
    timestamp(Utc::now())
}

pub(super) fn compact_now() -> String {
    Utc::now().format("%Y%m%dT%H%M%S%.fZ").to_string()
}

pub(super) fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}
