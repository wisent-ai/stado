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
//! Every probe is resolved through `PATH` ([`resolve_program`]) and never
//! from a path written here, because "which tools does this beacon have" is
//! the unit environment's answer, not this module's guess. The launchd plist
//! and the collector scripts carry the directories Tailscale installs into.

use std::time::Duration;

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{CommandOutput, CommandSpec, Runner};

/// Wall-clock cap on one probe. `pmset -g log` parses the whole power log
/// (measured 1.9 s over 36k lines on an M2 Max) and `log show` scans a log
/// store, so the cap is generous enough to succeed on a busy host and short
/// enough that a wedged tool costs the beacon one field, not the tick.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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

/// How far back the interface-change window reaches: one beacon interval, so
/// consecutive beacons tile the timeline without this module persisting a
/// cursor of its own.
fn window_seconds() -> i64 {
    std::env::var("WC_HEALTH_INTERVAL_SECONDS")
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_WINDOW_SECONDS)
        .clamp(MIN_WINDOW_SECONDS, MAX_WINDOW_SECONDS)
}

/// The window in whole minutes, which is the unit `log show --last` and
/// `journalctl --since` both take. Always at least one.
fn window_minutes(window: i64) -> i64 {
    (window + 59) / 60
}

/// Run one probe. `None` covers every way a probe can fail to answer:
/// missing binary, spawn error, timeout, non-zero exit.
async fn probe(runner: &Runner, argv: Vec<String>) -> Option<CommandOutput> {
    let mut spec = CommandSpec::new(argv);
    spec.timeout = Some(PROBE_TIMEOUT);
    match runner(spec).await {
        Ok(output) if output.ok() => Some(output),
        _ => None,
    }
}

/// The first executable named `name` on `PATH`, or `None`.
///
/// A beacon runs under launchd or systemd with whatever `PATH` its unit
/// declares, and `tailscale` lives in a different directory on every macOS
/// install (`/usr/local/bin`, Homebrew, inside the app bundle). Resolving
/// here — rather than trying a list of absolute paths — keeps that answer in
/// the unit environment, which is the only place that knows it.
fn resolve_program(name: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|directory| {
        if directory.as_os_str().is_empty() {
            return None;
        }
        let candidate = directory.join(name);
        let metadata = std::fs::metadata(&candidate).ok()?;
        (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .then(|| candidate.to_string_lossy().into_owned())
    })
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// One timestamp in the fleet's spelling: UTC, seconds, `Z`.
fn iso(stamp: DateTime<FixedOffset>) -> String {
    stamp
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parse the timestamp spellings the platform log tools print. Anything else
/// is skipped: a line whose time cannot be read is not evidence.
fn parse_stamp(raw: &str) -> Option<DateTime<FixedOffset>> {
    const FORMATS: [&str; 6] = [
        // pmset -g log: `2026-08-17 09:23:10 -0700`
        "%Y-%m-%d %H:%M:%S %z",
        // log show --style ndjson: `2026-08-19 11:59:32.869840-0700`
        "%Y-%m-%d %H:%M:%S%.f%z",
        // journalctl -o short-iso, as ubuntu-server spells it:
        // `2026-08-17T19:46:46+00:00`
        "%Y-%m-%dT%H:%M:%S%:z",
        // journalctl -o short-iso where the offset carries no colon
        "%Y-%m-%dT%H:%M:%S%z",
        // journalctl -o short-iso-precise, both offset spellings
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
    ];
    FORMATS
        .iter()
        .find_map(|format| DateTime::parse_from_str(raw.trim(), format).ok())
}

/// One log line's own sentence, flattened and truncated.
fn detail_of(raw: &str) -> String {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_DETAIL_CHARS {
        return flat;
    }
    flat.chars().take(MAX_DETAIL_CHARS).collect()
}

/// Keep the newest [`MAX_INTERFACE_CHANGES`], in log order.
fn newest_changes(mut changes: Vec<(DateTime<FixedOffset>, String)>) -> Vec<InterfaceChange> {
    changes.sort_by_key(|(stamp, _)| *stamp);
    let start = changes.len().saturating_sub(MAX_INTERFACE_CHANGES);
    changes
        .drain(start..)
        .map(|(stamp, detail)| InterfaceChange {
            at: iso(stamp),
            detail,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tailnet path
// ---------------------------------------------------------------------------

/// `(path_kind, endpoint)` from `tailscale status --json`, or `None` when
/// tailscale is absent or answered nothing parseable.
///
/// The host is reading about itself, and `Self.CurAddr` is empty on every
/// node (a node holds no path to itself), so directness is read from the
/// paths this host currently holds: a peer that is online with a `CurAddr` is
/// a direct path this host is party to. With no such peer, a node whose
/// backend is running still sits on its home DERP, which peers can reach it
/// through — that is a relay, not an absence.
///
/// `endpoint` stays about THIS host: its own dialable `ip:port` when direct
/// (the LAN spelling first, which is what one fleet on one network uses), and
/// `derp:<region>` when relayed. A peer's address is never published here as
/// if it were this host's.
async fn tailnet_path(runner: &Runner) -> Option<(String, Option<String>)> {
    let program = resolve_program("tailscale")?;
    let output = probe(
        runner,
        vec![program, "status".to_string(), "--json".to_string()],
    )
    .await?;
    let status: Value = serde_json::from_str(&output.stdout).ok()?;

    // Tailscale answered, so the source is real from here on; only the path
    // can still be unknown.
    if status.get("BackendState").and_then(Value::as_str) != Some("Running") {
        return Some((PATH_KIND_UNKNOWN.to_string(), None));
    }
    let node = status.get("Self");
    let direct = status
        .get("Peer")
        .and_then(Value::as_object)
        .is_some_and(|peers| {
            peers.values().any(|peer| {
                peer.get("Online").and_then(Value::as_bool) == Some(true)
                    && peer
                        .get("CurAddr")
                        .and_then(Value::as_str)
                        .is_some_and(|address| !address.trim().is_empty())
            })
        });
    if direct {
        return Some((PATH_KIND_DIRECT.to_string(), self_endpoint(node)));
    }
    let relay = node
        .and_then(|node| node.get("Relay"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|region| !region.is_empty());
    match relay {
        Some(region) => Some((PATH_KIND_RELAY.to_string(), Some(format!("derp:{region}")))),
        None => Some((PATH_KIND_UNKNOWN.to_string(), None)),
    }
}

/// This host's own direct endpoint out of `Self.CurAddr`/`Self.Addrs`.
///
/// A private address wins over a public one: the fleet shares a network, the
/// operator's evidence for the silence that started this was the LAN spelling
/// (`direct 10.0.0.253:41641`), and that is the endpoint a peer on the same
/// network actually dials. IPv6 entries are skipped — every reader of this
/// field so far quotes the `ip:port` form.
fn self_endpoint(node: Option<&Value>) -> Option<String> {
    let node = node?;
    if let Some(current) = node
        .get("CurAddr")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty())
    {
        return Some(current.to_string());
    }
    let addresses: Vec<&str> = node
        .get("Addrs")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|address| !address.is_empty() && !address.starts_with('['))
        .collect();
    let private = addresses.iter().find(|address| {
        address
            .rsplit_once(':')
            .and_then(|(host, _)| host.parse::<std::net::Ipv4Addr>().ok())
            .is_some_and(|address| address.is_private())
    });
    private
        .or_else(|| addresses.first())
        .map(|address| (*address).to_string())
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

/// The `pmset -g log` transition kinds that end a sleep. A DarkWake is a
/// maintenance wake rather than a user one, but it is still the moment the
/// network came back, which is the question being asked.
const MACOS_WAKE_KINDS: [&str; 2] = ["Wake", "DarkWake"];

/// `(last_sleep_at, last_wake_at)` from `pmset -g log`, or `None` when pmset
/// is absent or failed.
///
/// The read is bounded from the end: the log holds tens of thousands of lines
/// and only the newest transition of each kind is wanted, so the scan walks
/// backwards and stops as soon as it has both.
async fn macos_sleep_wake(runner: &Runner) -> Option<(Option<String>, Option<String>)> {
    let program = resolve_program("pmset")?;
    let output = probe(runner, vec![program, "-g".to_string(), "log".to_string()]).await?;

    let mut sleep = None;
    let mut wake = None;
    for line in output.stdout.lines().rev() {
        let mut fields = line.split_whitespace();
        let (Some(date), Some(time), Some(offset), Some(kind)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let is_sleep = kind == "Sleep";
        let is_wake = MACOS_WAKE_KINDS.contains(&kind);
        if (is_sleep && sleep.is_some()) || (is_wake && wake.is_some()) || (!is_sleep && !is_wake) {
            continue;
        }
        let Some(stamp) = parse_stamp(&format!("{date} {time} {offset}")) else {
            continue;
        };
        if is_sleep {
            sleep = Some(iso(stamp));
        } else {
            wake = Some(iso(stamp));
        }
        if sleep.is_some() && wake.is_some() {
            break;
        }
    }
    Some((sleep, wake))
}

/// The unified-log predicate for an interface change.
///
/// Anchored on `process == "configd"`, which is not decoration: an
/// unanchored `eventMessage CONTAINS` predicate over an eight hour window
/// took 2m52s on an M2 Max, while this one over three hours took 2.7s. The
/// process anchor is what lets the log store skip. `link` catches the link
/// state transitions and `SSID` the wifi association changes; the periodic
/// `publish success` heartbeat is deliberately not matched, because a
/// heartbeat is not a change.
const MACOS_INTERFACE_PREDICATE: &str =
    "process == \"configd\" AND (eventMessage CONTAINS \"link\" OR eventMessage CONTAINS \"SSID\")";

/// Interface changes inside the window from the unified log, or `None` when
/// `log` is absent or failed. `Some(vec![])` means the window was quiet.
async fn macos_interface_changes(runner: &Runner, window: i64) -> Option<Vec<InterfaceChange>> {
    let program = resolve_program("log")?;
    let output = probe(
        runner,
        vec![
            program,
            "show".to_string(),
            "--last".to_string(),
            format!("{}m", window_minutes(window)),
            "--style".to_string(),
            "ndjson".to_string(),
            "--predicate".to_string(),
            MACOS_INTERFACE_PREDICATE.to_string(),
        ],
    )
    .await?;

    let changes = output
        .stdout
        .lines()
        .filter_map(|line| {
            let entry: Value = serde_json::from_str(line.trim()).ok()?;
            let stamp = parse_stamp(entry.get("timestamp")?.as_str()?)?;
            let message = entry.get("eventMessage")?.as_str()?;
            (!message.trim().is_empty()).then(|| (stamp, detail_of(message)))
        })
        .collect();
    Some(newest_changes(changes))
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

/// `(last_sleep_at, last_wake_at)` from the journal's suspend unit, or `None`
/// when journalctl is absent or failed.
///
/// A registry Linux host is typically a server that never suspends, so both
/// halves being null is the normal, honest answer there.
async fn linux_sleep_wake(runner: &Runner) -> Option<(Option<String>, Option<String>)> {
    let program = resolve_program("journalctl")?;
    let output = probe(
        runner,
        vec![
            program,
            "--no-pager".to_string(),
            "-o".to_string(),
            "short-iso".to_string(),
            "-n".to_string(),
            "200".to_string(),
            "-u".to_string(),
            "systemd-suspend.service".to_string(),
        ],
    )
    .await?;

    let mut sleep = None;
    let mut wake = None;
    for line in output.stdout.lines().rev() {
        let Some(stamp) = line.split_whitespace().next().and_then(parse_stamp) else {
            continue;
        };
        // systemd's own wording for the unit that suspends the machine:
        // it starts as the host goes down and finishes as it comes back.
        if sleep.is_none() && line.contains("Starting") {
            sleep = Some(iso(stamp));
        } else if wake.is_none() && (line.contains("Finished") || line.contains("Stopped")) {
            wake = Some(iso(stamp));
        }
        if sleep.is_some() && wake.is_some() {
            break;
        }
    }
    Some((sleep, wake))
}

/// One journal line's own sentence: `journalctl -o short-iso` prefixes every
/// line with its timestamp and the hostname, and both are already answered by
/// the record around it (`at`, and the beacon's own host). What is left is
/// `kernel: ...`, which names the emitter and says what happened.
fn journal_message(line: &str) -> &str {
    line.splitn(3, ' ').nth(2).unwrap_or(line)
}

/// The kernel-log markers of a link transition, matched case-insensitively.
/// Copied from a live fleet host: `i40e ... eth0: NIC Link is Up, 1000 Mbps
/// Full Duplex` is the spelling a real interface change has there.
const LINUX_INTERFACE_MARKERS: [&str; 4] = [
    "link is up",
    "link is down",
    "link becomes ready",
    "carrier",
];

/// Interface changes inside the window from the kernel journal, or `None`
/// when journalctl is absent or failed.
async fn linux_interface_changes(runner: &Runner, window: i64) -> Option<Vec<InterfaceChange>> {
    let program = resolve_program("journalctl")?;
    let output = probe(
        runner,
        vec![
            program,
            "-k".to_string(),
            "--no-pager".to_string(),
            "-o".to_string(),
            "short-iso".to_string(),
            "--since".to_string(),
            format!("-{}min", window_minutes(window)),
            "-n".to_string(),
            "500".to_string(),
        ],
    )
    .await?;

    let changes = output
        .stdout
        .lines()
        .filter_map(|line| {
            let lowered = line.to_lowercase();
            if !LINUX_INTERFACE_MARKERS
                .iter()
                .any(|marker| lowered.contains(marker))
            {
                return None;
            }
            let stamp = line.split_whitespace().next().and_then(parse_stamp)?;
            Some((stamp, detail_of(journal_message(line))))
        })
        .collect();
    Some(newest_changes(changes))
}
