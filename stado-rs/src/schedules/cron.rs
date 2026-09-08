//! Due-time arithmetic: the croniter compatibility shim, the next-fire
//! computation the coordinator tick evaluates, and the ISO parsing that reads
//! a stored `next_due_at` back.
//!
//! The croniter deviation notes these functions implement — the 5-field
//! expansion, the day-of-week translation, the Vixie-cron OR semantics and the
//! corners that cannot match — are documented on [`crate::schedules`].

use std::collections::BTreeSet;
use std::str::FromStr;

use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;

/// Cron compilation failure (Python surfaces croniter's `CroniterBadCronError`
/// / `CroniterBadDateError` messages; here the message is ours).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CronError(pub String);

// ---------------------------------------------------------------------------
// cron compat shim (croniter 5-field → `cron` crate 6-field)
// ---------------------------------------------------------------------------

/// Raw croniter day-of-week value:
/// 0-6 for names/numbers, 7 for Sunday.
fn raw_dow_value(token: &str) -> Option<u8> {
    match token.to_ascii_lowercase().as_str() {
        "sun" | "sunday" => Some(0),
        "mon" | "monday" => Some(1),
        "tue" | "tues" | "tuesday" => Some(2),
        "wed" | "wednesday" => Some(3),
        "thu" | "thurs" | "thursday" => Some(4),
        "fri" | "friday" => Some(5),
        "sat" | "saturday" => Some(6),
        other => other.parse::<u8>().ok().filter(|n| *n <= 7),
    }
}

/// Expand a croniter day-of-week field to the `cron` crate's 1-7 numbering
/// (Sunday=1) as an explicit comma list. Returns `None` for syntax the shim
/// does not understand (callers then report the expression invalid).
fn expand_dow(field: &str) -> Option<String> {
    let mut days: BTreeSet<u8> = BTreeSet::new();
    for item in field.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return None;
        }
        let (base, step) = match item.split_once('/') {
            Some((base, step)) => (base, step.parse::<u8>().ok().filter(|s| *s > 0)?),
            None => (item, 1),
        };
        if base == "*" || base == "?" {
            for raw in (0u8..7).step_by(step as usize) {
                days.insert(raw);
            }
        } else if let Some((lo, hi)) = base.split_once('-') {
            let lo = raw_dow_value(lo)?;
            let hi = raw_dow_value(hi)?;
            // croniter tolerates wrap-around ranges ("6-1" wraps through
            // Sunday); iterate past 7 and fold with % 7.
            let hi = if hi < lo { hi + 7 } else { hi };
            let mut raw = lo;
            while raw <= hi {
                days.insert(raw % 7);
                raw += step;
            }
        } else {
            let value = raw_dow_value(base)?;
            if step == 1 {
                days.insert(value % 7);
            } else {
                // croniter treats "N/step" as N..6 stepped (e.g. "1/2" =
                // Mon, Wed, Fri).
                let mut raw = value;
                while raw <= 6 {
                    days.insert(raw % 7);
                    raw += step;
                }
            }
        }
    }
    if days.is_empty() {
        return None;
    }
    Some(
        days.iter()
            .map(|day| (day + 1).to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// True when the field matches every value (croniter treats `?` as `*`).
fn is_all(field: &str) -> bool {
    field == "*" || field == "?"
}

/// Compile a croniter 5-field expression (or @-alias) into one or two
/// `cron` crate schedules. Two schedules appear only for the dom+dow OR
/// case; the effective next-fire is the earliest of the two.
fn compile(cron: &str) -> Result<Vec<cron::Schedule>, CronError> {
    let invalid = || CronError(format!("invalid cron expression: {cron:?}"));
    let expr = cron.trim();
    // Aliases croniter accepts that the crate lacks (the other five —
    // @yearly/@monthly/@weekly/@daily/@hourly — parse natively).
    let expr = match expr {
        "@annually" => "@yearly",
        "@midnight" => "@daily",
        other => other,
    };
    if expr.starts_with('@') {
        return cron::Schedule::from_str(expr)
            .map(|schedule| vec![schedule])
            .map_err(|_| invalid());
    }
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(invalid());
    }
    let (minute, hour, dom, month, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
    let dow_translated = if is_all(dow) {
        "*".to_string()
    } else {
        expand_dow(dow).ok_or_else(invalid)?
    };
    let parse = |expr: String| cron::Schedule::from_str(&expr).map_err(|_| invalid());
    if !is_all(dom) && !is_all(dow) {
        // Vixie-cron OR semantics (croniter): restricted dom ORs with
        // restricted dow. The crate only ANDs, so compile both halves.
        Ok(vec![
            parse(format!("0 {minute} {hour} {dom} {month} *"))?,
            parse(format!("0 {minute} {hour} * {month} {dow_translated}"))?,
        ])
    } else {
        Ok(vec![parse(format!(
            "0 {minute} {hour} {dom} {month} {dow_translated}"
        ))?])
    }
}

/// True iff `cron` parses as a croniter expression (Python `cron_is_valid`).
///
/// See the module docs for the corners where this deliberately diverges
/// from croniter's `is_valid` (`L`, wrap ranges).
pub fn cron_is_valid(cron: &str) -> bool {
    compile(cron).is_ok()
}

/// First cron occurrence strictly after `after_utc`, returned as a UTC
/// datetime (Python `compute_next_due`).
///
/// The cron is interpreted in `tz` (so "0 2 * * *" means 02:00 in that
/// zone, DST included), then converted back to UTC for storage. An
/// unparseable `tz` falls back to UTC, exactly like Python's blanket
/// `except` around `ZoneInfo(tz)`.
pub fn compute_next_due(
    cron: &str,
    after_utc: DateTime<Utc>,
    tz: &str,
) -> Result<DateTime<Utc>, CronError> {
    let zone: Tz = tz.parse().unwrap_or(Tz::UTC);
    let base_local = after_utc.with_timezone(&zone);
    let mut best: Option<DateTime<Tz>> = None;
    for schedule in compile(cron)? {
        if let Some(next) = schedule.after(&base_local).next() {
            best = Some(match best {
                None => next,
                Some(current) => current.min(next),
            });
        }
    }
    best.map(|next| next.with_timezone(&Utc)).ok_or_else(|| {
        CronError(format!(
            "no future occurrence for cron expression: {cron:?}"
        ))
    })
}

/// Python `datetime.fromisoformat` for the shapes our writers produce
/// (offset-aware RFC-3339, or a naive value that gets UTC attached).
pub(in crate::schedules) fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|naive| naive.and_utc())
                .ok()
        })
}
