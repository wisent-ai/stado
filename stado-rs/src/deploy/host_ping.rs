//! `stado host ping TARGET` — one reachability verdict from two
//! independent signals.
//!
//! NO Python original: item three of `docs/missing-commands.md`.
//!
//! Why two signals and not one: on 2026-07-24 control-host answered
//! ssh perfectly while its health beacon was five days old — the disk was
//! full, launchd was wedged, and the beacon writer had not run since.
//! "Can I ssh in" and "is this box reporting" are different questions, and
//! a ping that answers only the first is exactly the tool that let that
//! incident run for five days. So both are probed, both are reported, and
//! the verdict is the WORSE of the two ([`Verdict`] is ordered worst-last
//! so the combination is a plain `max`).
//!
//! Signal one is the shared ssh channel ([`crate::deploy::host_channel`],
//! itself the option set of [`crate::deploy::host_reboot`]) running a
//! fixed, read-only remote program. Signal two is the beacon under
//! [`crate::monitor::host_health::HEALTH_PREFIX`], read through the
//! configured [`JobStorage`] backend by
//! [`crate::monitor::host_health::load_host_health`] — the same reader
//! `stado host health` uses, so the two commands can never disagree about
//! what the beacon says.

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Map, Value};

use super::host_channel;
use super::{DeployError, Runner};
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

/// How old a beacon may be before it counts as stale.
///
/// Both beacon writers — `deploy/host_health_beacon.sh` under a systemd
/// timer on Linux, the `com.wisent.host-health-beacon` LaunchAgent on
/// macOS — publish on a one-minute tick, the same cadence as the per-slot
/// heartbeat in [`crate::providers::local::slots`]. That heartbeat's
/// tolerance for a one-minute writer is already a settled number in this
/// crate ([`crate::config::HEARTBEAT_STALE_MINUTES`]), so it is reused
/// here rather than inventing a second answer to the same question. The
/// incident that motivated this command was five DAYS past this line, so
/// the exact tolerance was never the difficult part — having any at all
/// was.
pub fn beacon_stale_after() -> TimeDelta {
    TimeDelta::minutes(crate::config::HEARTBEAT_STALE_MINUTES)
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
    pub uri: Option<String>,
    /// Why the beacon is `down`, verbatim from the reader.
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
            "uri": self.uri,
            "error": self.error,
        })
    }
}

/// ISO-8601 parse for the two spellings involved: the `%Y-%m-%dT%H:%M:%SZ`
/// the beacon writers emit, and the offset form the storage layer reports
/// for `updated_at`. Both are RFC 3339.
fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
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
        return BeaconSignal {
            verdict: if age > beacon_stale_after() {
                Verdict::Stale
            } else {
                Verdict::Ok
            },
            reported_at: Some(raw.to_string()),
            source: Some(source.to_string()),
            age_seconds: Some(age.num_seconds()),
            uri,
            error: None,
        };
    }
    BeaconSignal {
        verdict: Verdict::Down,
        reported_at: None,
        source: None,
        age_seconds: None,
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
pub async fn ping_host(
    target_name: &str,
    store: &JobStorage,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let output = host_channel::run_program(&target, REMOTE_PROGRAM, runner).await?;

    let ssh_verdict = if output.ok() {
        Verdict::Ok
    } else {
        Verdict::Down
    };
    let beacon = beacon_signal(store, &target.name, Utc::now()).await;
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
    report.insert(
        "stale_after_seconds".to_string(),
        json!(beacon_stale_after().num_seconds()),
    );
    report
}
