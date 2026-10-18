//! The beacon's `link` block: a host's own account of its connectivity.
//!
//! The beacon a host publishes about itself is where the evidence of a
//! connectivity gap belongs: it is collected ON the host, so it can name the
//! sleep it just came out of and the tailnet path it holds right now, which
//! no reader elsewhere can see.
//!
//! Two rules shape everything here:
//!
//! - A beacon that does not publish would hide the gap this block exists to
//!   show, so [`collect_link`] cannot fail: a probe that fails leaves its
//!   datum null and its cause in `unread`.
//! - Where a datum cannot be read, the block says so and why. No default
//!   ever stands in for a measurement: a host without `pmset` publishes a
//!   null sleep time, not "never slept", and a host that is neither macOS
//!   nor Linux reports `source: "unsupported"` rather than a fabricated path.
//!
//! Every probe is resolved through `PATH` (`resolve_program`) and never
//! from a path written here, because "which tools does this beacon have" is
//! the unit environment's answer, not this module's guess. The launchd plist
//! and the collector scripts carry the directories Tailscale installs into.

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
    /// The log line's own sentence, flattened to one line. Never reworded:
    /// a reader diagnosing a silence
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
    /// Every datum above the host could not read, each with its cause. A
    /// null with no entry here is a datum the host has no tool for (no
    /// tailscale, no pmset), not one it failed to read. Null in a beacon
    /// from a Stado that did not report it.
    #[serde(default)]
    pub unread: Option<Vec<String>>,
}

impl BeaconLink {
    /// The block a host publishes when nothing about its link can be read:
    /// every datum null, named as unsupported. Collected at is still real.
    pub fn unsupported(collected_at: String, unread: Vec<String>) -> Self {
        Self {
            collected_at,
            path_kind: PATH_KIND_UNKNOWN.to_string(),
            endpoint: None,
            last_sleep_at: None,
            last_wake_at: None,
            interface_changes: Vec::new(),
            source: SOURCE_UNSUPPORTED.to_string(),
            unread: Some(unread),
        }
    }

    /// What a loaded beacon document published for its link: none when the
    /// beacon predates the block, the block, or — when it carries something
    /// that is not this shape — why, in the block's own `unread` field. This
    /// never invents a block a host did not publish.
    pub fn from_beacon(beacon: &Map<String, Value>) -> Option<Published> {
        let link = beacon.get("link")?;
        Some(match serde_json::from_value(link.clone()) {
            Ok(block) => Published::Block(block),
            Err(error) => Published::Unreadable {
                unread: vec![format!("the beacon's link block is not one: {error}")],
            },
        })
    }
}

/// What one beacon published for its link, as a reader shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Published {
    /// The block the host collected.
    Block(BeaconLink),
    /// A link block that could not be read, and why.
    Unreadable { unread: Vec<String> },
}

/// One probe's answer: its datum, or its cause recorded in `unread` and no
/// datum.
fn read<T>(unread: &mut Vec<String>, what: &str, answer: Result<Option<T>, String>) -> Option<T> {
    match answer {
        Ok(datum) => datum,
        Err(cause) => {
            unread.push(format!("{what}: {cause}"));
            None
        }
    }
}

/// Collect this host's `link` block. Never fails: an unreadable datum is a
/// null with its cause in `unread`, and an unknown platform is
/// [`SOURCE_UNSUPPORTED`].
pub async fn collect_link(runner: &Runner) -> BeaconLink {
    let collected_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut unread = Vec::new();
    let window = match window_seconds() {
        Ok(Some(window)) => Some(window),
        Ok(None) => {
            unread.push(
                "interface changes: the beacon unit declares no WC_HEALTH_INTERVAL_SECONDS, so \
                 no window was read"
                    .to_string(),
            );
            None
        }
        Err(cause) => {
            unread.push(format!("interface changes: {cause}"));
            None
        }
    };
    let path = read(&mut unread, "tailnet path", tailnet_path(runner).await);

    let (sleep_wake, changes, platform_source) = match std::env::consts::OS {
        "macos" => (
            read(
                &mut unread,
                "sleep and wake",
                macos_sleep_wake(runner).await,
            ),
            match window {
                Some(window) => read(
                    &mut unread,
                    "interface changes",
                    macos_interface_changes(runner, window).await,
                ),
                None => None,
            },
            SOURCE_MACOS,
        ),
        "linux" => (
            read(
                &mut unread,
                "sleep and wake",
                linux_sleep_wake(runner).await,
            ),
            match window {
                Some(window) => read(
                    &mut unread,
                    "interface changes",
                    linux_interface_changes(runner, window).await,
                ),
                None => None,
            },
            SOURCE_LINUX,
        ),
        // A platform whose sleep log this module has never read refuses to
        // name a source it does not have.
        _ => return BeaconLink::unsupported(collected_at, unread),
    };

    // What decides the source is which probes ANSWERED, not what they found:
    // a quiet host that has never slept read its log successfully, while a
    // host with no `pmset` and no `tailscale` published a block holding no
    // measurement at all, and calling that one `pmset+tailscale` would claim
    // two readings nobody took.
    let answered = path.is_some() || sleep_wake.is_some() || changes.is_some();
    if !answered {
        return BeaconLink::unsupported(collected_at, unread);
    }
    let (last_sleep_at, last_wake_at) = match sleep_wake {
        Some(halves) => halves,
        None => (None, None),
    };
    let (path_kind, endpoint) = match path {
        Some(path) => path,
        None => (PATH_KIND_UNKNOWN.to_string(), None),
    };
    BeaconLink {
        collected_at,
        path_kind,
        endpoint,
        last_sleep_at,
        last_wake_at,
        interface_changes: match changes {
            Some(changes) => changes,
            None => Vec::new(),
        },
        source: platform_source.to_string(),
        unread: Some(unread),
    }
}
