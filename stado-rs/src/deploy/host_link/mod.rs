//! The beacon's `link` block: a host's own account of its connectivity.
//!
//! A connectivity gap used to leave no trace in this product. On 2026-08-19 a
//! fleet Mac went unreachable for six minutes — 100% ping loss, ssh timing
//! out, then `direct 10.0.0.253:41641` back with 13–215 ms — and the only
//! evidence anywhere was the two ping packets an operator happened to send.
//! The beacon a host publishes about itself is where that evidence belongs:
//! it is collected ON the host, so it can name the sleep it just came out of
//! and the tailnet path it holds right now. No reader can see either.
//!
//! Two rules shape everything here:
//!
//! - A beacon that does not publish is the exact failure this block exists to
//!   remove, so every external command is capped at [`PROBE_TIMEOUT`] and
//!   every failure degrades to a null. [`collect_link`] cannot fail; it can
//!   only come back thinner.
//! - Where a datum cannot be read, the block says so. No default ever stands
//!   in for a measurement: an absent `pmset` yields a null sleep time, not
//!   "never slept", and a host that is neither macOS nor Linux reports
//!   `source: "unsupported"` rather than a fabricated path.
//!
//! Every probe is resolved through `PATH` (`resolve_program`) and never
//! from a path written here, because "which tools does this beacon have" is
//! the unit environment's answer, not this module's guess. The launchd plist
//! and the collector scripts carry the directories Tailscale installs into.

use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::Runner;

mod platform;
mod probes;
mod tailnet;
mod timestamps;

use platform::{
    linux_interface_changes, linux_sleep_wake, macos_interface_changes, macos_sleep_wake,
};
use probes::window_seconds;
use tailnet::tailnet_path;

/// Wall-clock cap on one probe. `log show` scans a log store, so the cap is
/// generous enough to succeed on a busy host and short enough that a wedged
/// tool costs the beacon one field, not the tick.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Wall-clock cap on reading the power log, which is a different measurement
/// from the one above and was wrong for two years of log growth.
///
/// The comment on `PROBE_TIMEOUT` recorded `pmset -g log` at 1.9 s over 36k
/// lines. On 2026-09-08 the same command on this workstation took 6.03 s, so
/// the five second cap killed it and the beacon carried no sleep or wake at
/// all — the two fields `host link` exists to answer with. The log only grows,
/// so the cap has to leave room for that growth rather than sit beside the
/// measurement it was taken from.
pub const POWER_LOG_TIMEOUT: Duration = Duration::from_secs(30);

/// Interface changes one beacon carries. The window is minutes long; a host
/// flapping harder than this is telling its story with the first few lines,
/// and an unbounded list would grow the document without bound.
const MAX_INTERFACE_CHANGES: usize = 8;

/// Longest `detail` sentence kept, in characters. Matches the truncation the
/// recovery channel already applies to captured tool output.
const MAX_DETAIL_CHARS: usize = 160;

/// Log window when the beacon interval is unset: the beacon's own default
/// cadence, so "since the previous beacon" needs no persisted state.
const DEFAULT_WINDOW_SECONDS: i64 = 300;

/// Window bounds. Below a minute the window misses the change that silenced
/// the host; above a quarter hour `log show` stops being cheap.
const MIN_WINDOW_SECONDS: i64 = 60;
const MAX_WINDOW_SECONDS: i64 = 900;

/// The host holds at least one direct path to the tailnet.
pub const PATH_KIND_DIRECT: &str = "direct";
/// Every path the host holds runs through a DERP relay.
pub const PATH_KIND_RELAY: &str = "relay";
/// Tailscale is absent, not running, or answered nothing usable.
pub const PATH_KIND_UNKNOWN: &str = "unknown";

/// macOS: `pmset -g log` for sleep/wake, `tailscale status --json` for the
/// path, `log show` for interface changes.
pub const SOURCE_MACOS: &str = "pmset+tailscale";
/// Linux: `journalctl` for suspend/resume, `tailscale status --json` for the
/// path, `journalctl -k` for interface changes.
pub const SOURCE_LINUX: &str = "journalctl+tailscale";
/// Nothing on this host could be read: not the platform's log tool, not
/// tailscale. Every field is null and the reader is told so by name.
pub const SOURCE_UNSUPPORTED: &str = "unsupported";

/// One interface transition inside the collection window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceChange {
    /// When the platform log recorded it (UTC, seconds).
    pub at: String,
    /// The log line's own sentence, flattened to one line and truncated to
    /// [`MAX_DETAIL_CHARS`]. Never reworded: a reader diagnosing a silence
    /// needs the wording the machine used.
    pub detail: String,
}

/// The `link` block of one host health beacon.
///
/// Field order is the published order. `Option` fields serialize as `null`
/// rather than disappearing, because "we could not read it" is the answer a
/// reader must be able to see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeaconLink {
    /// When this block was collected (UTC, seconds). Always present, even
    /// when every probe failed — it dates the failure.
    pub collected_at: String,
    /// [`PATH_KIND_DIRECT`], [`PATH_KIND_RELAY`] or [`PATH_KIND_UNKNOWN`].
    pub path_kind: String,
    /// Where peers reach this host on the path it holds: its own `ip:port`
    /// when direct, `derp:<region>` when relayed, null when unknown.
    pub endpoint: Option<String>,
    /// Newest sleep/suspend transition the platform log carries.
    pub last_sleep_at: Option<String>,
    /// Newest wake/resume transition the platform log carries.
    pub last_wake_at: Option<String>,
    /// Interface changes inside the window, oldest first. An empty list is a
    /// legitimate answer: a quiet window is the common case.
    pub interface_changes: Vec<InterfaceChange>,
    /// [`SOURCE_MACOS`], [`SOURCE_LINUX`] or [`SOURCE_UNSUPPORTED`].
    pub source: String,
}

impl BeaconLink {
    /// The block a host publishes when nothing about its link can be read:
    /// every datum null, named as unsupported. Collected at is still real.
    pub fn unsupported(collected_at: String) -> Self {
        Self {
            collected_at,
            path_kind: PATH_KIND_UNKNOWN.to_string(),
            endpoint: None,
            last_sleep_at: None,
            last_wake_at: None,
            interface_changes: Vec::new(),
            source: SOURCE_UNSUPPORTED.to_string(),
        }
    }

    /// The block carried by a loaded beacon document, or `None` when the
    /// beacon predates it or carries something that is not this shape. A
    /// reader renders `None` as the unsupported/unknown nulls itself; this
    /// never invents a block a host did not publish.
    pub fn from_beacon(beacon: &Map<String, Value>) -> Option<Self> {
        serde_json::from_value(beacon.get("link")?.clone()).ok()
    }
}

/// Collect this host's `link` block. Never fails: an unreadable datum is a
/// null and an unknown platform is [`SOURCE_UNSUPPORTED`].
pub async fn collect_link(runner: &Runner) -> BeaconLink {
    let collected_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let window = window_seconds();
    let path = tailnet_path(runner).await;

    let (sleep_wake, changes, platform_source) = match std::env::consts::OS {
        "macos" => (
            macos_sleep_wake(runner).await,
            macos_interface_changes(runner, window).await,
            SOURCE_MACOS,
        ),
        "linux" => (
            linux_sleep_wake(runner).await,
            linux_interface_changes(runner, window).await,
            SOURCE_LINUX,
        ),
        // A platform whose sleep log this module has never read reports the
        // tailnet path it can read and refuses to name a source it does not
        // have.
        _ => return BeaconLink::unsupported(collected_at),
    };

    // What decides the source is which probes ANSWERED, not what they found:
    // a quiet host that has never slept read its log successfully, while a
    // host with no `pmset` and no `tailscale` published a block holding no
    // measurement at all, and calling that one `pmset+tailscale` would claim
    // two readings nobody took.
    let answered = path.is_some() || sleep_wake.is_some() || changes.is_some();
    if !answered {
        return BeaconLink::unsupported(collected_at);
    }
    let (last_sleep_at, last_wake_at) = sleep_wake.unwrap_or((None, None));
    let (path_kind, endpoint) = path.unwrap_or((PATH_KIND_UNKNOWN.to_string(), None));
    BeaconLink {
        collected_at,
        path_kind,
        endpoint,
        last_sleep_at,
        last_wake_at,
        interface_changes: changes.unwrap_or_default(),
        source: platform_source.to_string(),
    }
}
