//! The per-job heartbeat blob: the timestamp embedded in it, and the
//! freshness question the reaper asks of a VM's jids.

use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;

use crate::queue::JobStorage;

use super::{now_unix, unix_seconds};

static TS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?)")
        .expect("static regex compiles")
});

/// Parse an ISO-8601 timestamp from the heartbeat blob and return
/// unix seconds. The agent writes lines like:
///     RUNNING 2026-05-13T00:26:33.130155+00:00
/// Returns None if no parseable timestamp is found.
fn parse_heartbeat_ts(text: &str) -> Option<f64> {
    if text.is_empty() {
        return None;
    }
    let caps = TS_RE.captures(text)?;
    let mut raw = caps[1].replace(' ', "T");
    // Python rstrip("Z").
    while raw.ends_with('Z') {
        raw.pop();
    }
    if raw.contains('.') {
        let dot = raw.find('.').expect("contains checked");
        let head = &raw[..dot];
        let frac = &raw[dot + 1..];
        // First +/- in the fraction starts the timezone.
        let (frac_digits, tz) = match frac.find(['+', '-']) {
            Some(i) => (&frac[..i], &frac[i..]),
            None => (frac, ""),
        };
        let frac_digits: String = frac_digits.chars().take(6).collect();
        raw = format!("{head}.{frac_digits}{tz}");
    }
    if !raw.contains('+') && !raw.ends_with('Z') {
        raw.push_str("+00:00");
    }
    // Python `datetime.fromisoformat(raw)`; after the normalization above
    // the string is always RFC3339-shaped. A chrono rejection maps to None
    // (Python would raise ValueError, but only on inputs the regex +
    // normalization can produce for negative-offset zones, which production
    // never writes — heartbeats are always UTC).
    let dt = DateTime::parse_from_rfc3339(&raw).ok()?;
    Some(unix_seconds(dt.with_timezone(&Utc)))
}

/// True iff ANY job in jids has a heartbeat blob whose embedded
/// timestamp is younger than threshold_seconds. Used by the reaper:
/// if the agent's capacity blob is stale but a job assigned to its
/// VM is still heartbeating, the agent is alive — busy in the
/// training subprocess — and the VM should NOT be deleted.
pub async fn any_job_heartbeat_fresh(
    store: &JobStorage,
    jids: &[String],
    threshold_seconds: f64,
) -> bool {
    let now = now_unix();
    for jid in jids {
        if jid.is_empty() {
            continue;
        }
        let text = match store
            .download_text(&format!("status/{jid}/heartbeat"))
            .await
        {
            Ok(text) => text,
            Err(_) => {
                // A coordinator-side GCS read failure is NOT proof the job
                // is dead. The old `except: text=None` path made a transient
                // Cloud-Function storage hiccup on one monitor tick read as
                // "no heartbeat" for EVERY running job, requeuing them all
                // in the same pass — the synchronized orphan churn (3ef705b2
                // + 724084db both requeued 2026-05-16T04:09:19, restart 11,
                // on freshly-written heartbeat blobs). Fail safe: a read
                // error defers (treat as alive); never let the coordinator's
                // own read failure destroy a running job. A genuinely dead
                // job is still caught when the read succeeds (stale ts) or
                // via the TERMINATED/absent VM path.
                return true;
            }
        };
        let Some(text) = text.filter(|t| !t.is_empty()) else {
            continue;
        };
        let Some(ts) = parse_heartbeat_ts(&text) else {
            continue;
        };
        if now - ts < threshold_seconds {
            return true;
        }
    }
    false
}
