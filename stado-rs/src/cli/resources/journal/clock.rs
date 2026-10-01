//! The clock every archived document is stamped from: the wire timestamp a
//! record carries, and the compact stamp an event file is named by.

use chrono::{DateTime, SecondsFormat, Utc};

pub(super) fn now() -> String {
    timestamp(Utc::now())
}

pub(super) fn compact_now() -> String {
    Utc::now().format("%Y%m%dT%H%M%S%.fZ").to_string()
}

pub(super) fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}
