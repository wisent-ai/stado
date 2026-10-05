//! `stado host ping TARGET` — one reachability verdict from two
//! independent signals.
//!
//! NO Python original: item three of `stado.wisent.com/docs/missing-commands`.
//!
//! SSH reachability does not prove that the health-beacon writer is working.
//! Probe and report both signals. [`Verdict`] orders worse states last, so
//! their combined verdict is their maximum.
//!
//! Signal one is the shared ssh channel ([`crate::deploy::host_channel`],
//! itself the option set of [`crate::deploy::host_state::reboot`]) running a
//! fixed, read-only remote program. Signal two is the beacon under
//! [`crate::monitor::host_health::HEALTH_PREFIX`], read through the
//! configured [`JobStorage`] backend by
//! [`crate::monitor::host_health::load_host_health`] — the same reader
//! `stado host health` uses, so the two commands can never disagree about
//! what the beacon says.

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Map, Value};

use crate::deploy::host_channel;
use crate::deploy::{DeployError, Runner};
use crate::monitor::host_health::{self, HostHealthReport};
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

/// The fixed remote program for the ssh signal: print the short hostname.
///
/// It has to be a real program rather than an empty command, because an
/// ssh session that authenticates but whose login shell then fails to
/// start is exactly the half-dead state this command exists to catch. The
/// hostname it prints is also the cheapest confirmation that the
/// destination the registry holds still resolves to the box it names.
pub const REMOTE_PROGRAM: &[&str] = &["/bin/hostname", "-s"];

/// How the two signals rank against each other. Declaration order IS the
/// severity order, so `Ord`/`max` composes the verdict without a table of
/// numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    /// The signal is healthy.
    Ok,
    /// The signal answered but is out of date.
    Stale,
    /// The signal did not answer at all.
    Down,
}

impl Verdict {
    /// The wire spelling, and the `status` field of the report.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Stale => "stale",
            Self::Down => "down",
        }
    }
}

/// The time by which the beacon's publisher promised the next one: the
/// `next_by` it wrote (`stado serve --health-interval-seconds`, its own
/// period plus its last collection), or for a beacon from before that field,
/// `stamp` plus the `stale_after_seconds` it stated. `None` when it states
/// neither: a beacon handed in by a one-shot command promises nothing.
pub fn beacon_next_by(beacon: &Map<String, Value>, stamp: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if let Some(next) = beacon
        .get("next_by")
        .and_then(Value::as_str)
        .and_then(parse_timestamp)
    {
        return Some(next);
    }
    let window = beacon.get("stale_after_seconds").and_then(Value::as_i64)?;
    Some(stamp + TimeDelta::seconds(window))
}

/// The beacon half of the verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct BeaconSignal {
    pub verdict: Verdict,
    /// The timestamp the age was measured from, when there was one.
    pub reported_at: Option<String>,
    /// Which field supplied it: the beacon's own `reported_at`, or the
    /// storage object's `updated_at` when the beacon omits it.
    pub source: Option<String>,
    pub age_seconds: Option<i64>,
    /// When the publisher promised its next beacon, if it said.
    pub next_by: Option<String>,
    pub uri: Option<String>,
    /// Why the beacon is `down` or `stale`, verbatim from the reader.
    pub error: Option<String>,
}

impl BeaconSignal {
    /// A beacon that could not be read at all.
    pub fn unreadable(error: String) -> Self {
        Self {
            verdict: Verdict::Down,
            reported_at: None,
            source: None,
            age_seconds: None,
            next_by: None,
            uri: None,
            error: Some(error),
        }
    }

    /// The signal as its report section.
    pub fn to_value(&self) -> Value {
        json!({
            "status": self.verdict.as_str(),
            "reported_at": self.reported_at,
            "source": self.source,
            "age_seconds": self.age_seconds,
            "next_by": self.next_by,
            "uri": self.uri,
            "error": self.error,
        })
    }
}

/// ISO-8601 parse for the two spellings involved: the `%Y-%m-%dT%H:%M:%SZ`
/// the beacon writers emit, and the offset form the storage layer reports
/// for `updated_at`. Both are RFC 3339.
///
/// Public because [`BeaconSignal::reported_at`] hands back the raw string it
/// aged, and a caller that needs the instant — `stado host link`, recording
/// when a host was last heard from — must recover it with the same parser that
/// accepted it. Deriving the instant from `age_seconds` instead would put a
/// whole second of rounding into a record whose whole purpose is when the
/// silence began.
pub fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

/// Grade a beacon that WAS read: the beacon's own `reported_at` if it
/// carries one, else the storage object's `updated_at`.
///
/// A beacon present but carrying no usable timestamp is `down`, not `ok` —
/// an unaged beacon proves nothing about whether the box is still
/// reporting, which is the entire question being asked.
pub fn grade_beacon(report: &HostHealthReport, now: DateTime<Utc>) -> BeaconSignal {
    let uri = report
        .object
        .get("uri")
        .and_then(Value::as_str)
        .map(str::to_string);
    let candidates = [
        ("reported_at", report.beacon.get("reported_at")),
        ("object_updated_at", report.object.get("updated_at")),
    ];
    for (source, value) in candidates {
        let Some(raw) = value.and_then(Value::as_str) else {
            continue;
        };
        let Some(stamp) = parse_timestamp(raw) else {
            continue;
        };
        let age = now.signed_duration_since(stamp);
        let next_by = beacon_next_by(&report.beacon, stamp);
        let (verdict, error) = match next_by {
            Some(by) if now <= by => (Verdict::Ok, None),
            Some(_) => (Verdict::Stale, None),
            None => (
                Verdict::Stale,
                Some(
                    "the beacon states no time for its next publication (neither next_by nor \
                     stale_after_seconds), so nothing says it is still current"
                        .to_string(),
                ),
            ),
        };
        return BeaconSignal {
            verdict,
            reported_at: Some(raw.to_string()),
            source: Some(source.to_string()),
            age_seconds: Some(age.num_seconds()),
            next_by: next_by.map(|by| by.to_rfc3339()),
            uri,
            error,
        };
    }
    BeaconSignal {
        verdict: Verdict::Down,
        reported_at: None,
        source: None,
        age_seconds: None,
        next_by: None,
        uri,
        error: Some("beacon carries no parseable timestamp".to_string()),
    }
}

/// Read and grade the beacon for one identity.
pub async fn beacon_signal(store: &JobStorage, identity: &str, now: DateTime<Utc>) -> BeaconSignal {
    match host_health::load_host_health(store, identity).await {
        Ok(report) => grade_beacon(&report, now),
        // Every failure mode here — no beacon object at all, unparseable
        // JSON, an unreachable store — means the same thing to an
        // operator: this box is not reporting. The reader's own message
        // says which, so it is passed through untouched.
        Err(exc) => BeaconSignal::unreadable(exc.to_string()),
    }
}

/// Probe both signals and combine them into one verdict.
///
/// `store` is the beacon store as it could be opened, or why it could not.
/// A store that does not open is the beacon half's answer, not the whole
/// command's: a `stado host ping` that exits on a closed object API alone
/// leaves nobody able to ask whether the host itself answers ssh.
pub async fn ping_host(
    target_name: &str,
    store: Result<&JobStorage, String>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let output = host_channel::run_program(&target, REMOTE_PROGRAM, runner).await?;

    let ssh_verdict = if output.ok() {
        Verdict::Ok
    } else {
        Verdict::Down
    };
    let beacon = match store {
        Ok(store) => beacon_signal(store, &target.name, Utc::now()).await,
        Err(error) => BeaconSignal::unreadable(format!("the beacon store did not open: {error}")),
    };
    let verdict = ssh_verdict.max(beacon.verdict);

    let mut report = build_report(&target, &output.stdout, ssh_verdict, &beacon);
    // finish_report supplies exit_code and the last stderr line, then the
    // combined verdict overwrites its per-command status: a box that
    // answers ssh is not "ok" when nothing has heard from it in days.
    host_channel::finish_report(&mut report, &output, verdict.as_str(), "ssh failed");
    report.insert("status".to_string(), json!(verdict.as_str()));
    Ok(Value::Object(report))
}

/// Assemble the report body (everything except `exit_code` / `status`,
/// which [`host_channel::finish_report`] owns).
fn build_report(
    target: &ComputeTarget,
    ssh_stdout: &str,
    ssh_verdict: Verdict,
    beacon: &BeaconSignal,
) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert(
        "ssh_check".to_string(),
        json!({
            "status": ssh_verdict.as_str(),
            "host": ssh_stdout.trim(),
        }),
    );
    report.insert("beacon".to_string(), beacon.to_value());
    report
}
