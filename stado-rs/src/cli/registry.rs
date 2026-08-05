//! `stado registry validate|push|pull|self|doctor|host add|beacon-age` —
//! canonical registry management.
//!
//! `validate`, `push` and `pull` port the `registry` group of
//! `stado/cli.py`. `self`, `doctor`, `host add` and `beacon-age` have NO
//! Python original: they close items fifteen through seventeen of
//! `docs/missing-commands.md`, written after the 2026-07-24
//! charless-mac-mini incident, where the registry declared a host that
//! nothing on the box was honouring and no command could say so.
//!
//! Every read and write goes through [`targets::RegistryStore`], so the
//! group repairs the registry on whichever store `WC_STORAGE_BACKEND`
//! selects. It used to hardcode `gs://wisent-compute/registry.json` and
//! build a `GcsBackend` directly, which on an Azure-only deployment meant
//! the one document the coordinator's survival check reads
//! (`targets::fetch_registry_remote`) could be repaired by nobody.
//!
//! [`doctor`] and [`beacon_age`] source liveness from the host beacons
//! (`monitor/host_health.rs`) and the capacity broadcasts
//! (`queue/capacity.rs`), never from ssh, so they cost one prefix listing
//! and are safe to run on a loop.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Map, Value};

use crate::monitor::host_health;
use crate::queue::{capacity, JobStorage};
use crate::targets::{
    self, bundled_registry_path, validate_registry, validate_registry_file, ComputeTarget,
    Registry, RegistryStore,
};

use super::table;
use super::CmdError;

/// The state a live launchd/systemd unit reports
/// (`deploy/host_health_beacon_macos.sh`, `deploy/host_health_beacon.sh`).
const ACTIVE_STATE: &str = "active";

fn source_path(path: Option<String>) -> PathBuf {
    path.map(PathBuf::from)
        .unwrap_or_else(bundled_registry_path)
}

pub fn validate(path: Option<String>) -> Result<(), CmdError> {
    let source = source_path(path);
    validate_registry_file(&source).map_err(|exc| CmdError::click(exc.to_string()))?;
    println!("valid registry: {}", source.display());
    Ok(())
}

/// Top-level keys the outgoing document would delete from the object that is
/// already there.
///
/// A registry write is a whole-document replace, so a caller holding a stale
/// or differently-modelled copy silently deletes every key its own model does
/// not know about. That is not hypothetical: on 2026-08-04 the canonical
/// document lost `channels`, `enrollment` and `fleets` between one read and
/// the next, and gained a `service_directory` block that no checkout in the
/// tree models — divergent builds writing the same object, each erasing what
/// it could not name. `fetch_document` exists so read-modify-write callers
/// keep the raw document; this is the backstop for everyone who does not.
///
/// Only removals are reported. Additions are how the document grows, and a
/// changed value is an edit rather than a loss.
fn removed_top_level_keys(current: &str, payload: &str) -> Vec<String> {
    let (Ok(Value::Object(before)), Ok(Value::Object(after))) = (
        serde_json::from_str::<Value>(current),
        serde_json::from_str::<Value>(payload),
    ) else {
        return Vec::new();
    };
    before
        .keys()
        .filter(|key| !after.contains_key(*key))
        .cloned()
        .collect()
}

/// The upload half [`push`] and [`push_document`] share: read the current
/// generation, refuse a write that would delete a top-level key unless the
/// operator said so, compare-and-swap against it (or atomically create when
/// the object is absent), then read back and verify BOTH the generation and
/// the bytes. Returns `(generation, previous_generation)`.
///
/// `payload` is written verbatim, so [`push`] still uploads the operator's
/// exact file bytes rather than a re-serialization of them.
async fn upload_payload(payload: &str, allow_removals: bool) -> Result<(String, String), CmdError> {
    let store = RegistryStore::open()
        .await
        .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?;
    let current = store
        .read_versioned()
        .await
        .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?;
    let previous_generation = current
        .as_ref()
        .map(|blob| blob.version.clone())
        .unwrap_or_else(|| "0".to_string());
    if !allow_removals {
        if let Some(blob) = current.as_ref() {
            let removed = removed_top_level_keys(&blob.content, payload);
            if !removed.is_empty() {
                return Err(CmdError::click(format!(
                    "registry upload refused: it would delete the top-level key(s) {} \
                     that generation {} carries. A registry write replaces the whole \
                     document, so this is what a stale copy or a build that does not \
                     model those keys does to them. Re-pull, re-apply the edit, and \
                     push again; pass --force only if the deletion is the intent.",
                    removed.join(", "),
                    blob.version
                )));
            }
        }
    }
    let generation = match current {
        Some(blob) => store
            .compare_and_swap(&blob.version, payload)
            .await
            .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?,
        None => {
            let created = store
                .create_if_absent(payload)
                .await
                .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?;
            if !created {
                return Err(CmdError::click("registry upload failed: concurrent create"));
            }
            store
                .read_versioned()
                .await
                .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?
                .ok_or_else(|| {
                    CmdError::click("registry upload verification could not read the object")
                })?
                .version
        }
    };
    let confirmed = store
        .read_versioned()
        .await
        .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?
        .ok_or_else(|| CmdError::click("registry upload verification could not read the object"))?;
    if confirmed.version != generation || confirmed.content != payload {
        return Err(CmdError::click(
            "registry upload verification returned different bytes",
        ));
    }
    Ok((generation, previous_generation))
}

pub async fn push(path: Option<String>, force: bool) -> Result<(), CmdError> {
    let source = source_path(path);
    validate_registry_file(&source).map_err(|exc| CmdError::click(exc.to_string()))?;
    let payload = std::fs::read_to_string(&source)?;
    let (generation, previous_generation) = upload_payload(&payload, force).await?;
    println!(
        "pushed {} -> {} generation={generation} replaced={previous_generation}",
        source.display(),
        targets::registry_location()
    );
    Ok(())
}

/// Validate an in-memory registry document and write it through the same
/// compare-and-swap [`push`] performs; returns the new generation.
///
/// The single validated write path for programmatic registry edits —
/// `deploy/service.rs` (`stado service adopt|retire|deploy`) and
/// [`host_add`] both land here. Validation runs BEFORE any store call, so
/// a document that would not validate never reaches the registry.
pub async fn push_document(document: &Value) -> Result<String, CmdError> {
    validate_registry(document).map_err(|exc| CmdError::click(exc.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(document)?);
    let (generation, _) = upload_payload(&payload, false).await?;
    Ok(generation)
}
pub async fn push_document_if(
    document: &Value,
    expected_generation: &str,
) -> Result<String, CmdError> {
    validate_registry(document).map_err(|exc| CmdError::click(exc.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(document)?);
    let store = RegistryStore::open().await?;
    let generation = store
        .compare_and_swap(expected_generation, &payload)
        .await
        .map_err(|error| CmdError::click(format!("registry compare-and-swap failed: {error}")))?;
    let confirmed = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click("registry compare-and-swap verification found no object"))?;
    if confirmed.version != generation || confirmed.content != payload {
        return Err(CmdError::click(
            "registry compare-and-swap verification returned different bytes",
        ));
    }
    Ok(generation)
}

pub async fn fetch_versioned_document() -> Result<(Value, String), CmdError> {
    let store = RegistryStore::open().await?;
    let blob = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click(format!("no registry document at {}", store.location())))?;
    let document: Value = serde_json::from_str(&blob.content)?;
    if !document.is_object() {
        return Err(CmdError::click(format!(
            "registry at {} is not an object",
            store.location()
        )));
    }
    Ok((document, blob.version))
}

/// The canonical registry as its raw document, off the same object
/// [`push_document`] compare-and-swaps.
///
/// Read-modify-write callers need this rather than
/// [`targets::fetch_registry_remote`]: [`Registry`] drops every key it
/// does not model, so serializing it back would silently delete them.
pub async fn fetch_document() -> Result<Value, CmdError> {
    let store = RegistryStore::open().await?;
    let text = store
        .read_text()
        .await?
        .ok_or_else(|| CmdError::click(format!("no registry document at {}", store.location())))?;
    let document: Value = serde_json::from_str(&text)?;
    if !document.is_object() {
        return Err(CmdError::click(format!(
            "registry at {} is not an object",
            store.location()
        )));
    }
    Ok(document)
}

pub async fn pull() -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let text = store.read_text().await?.ok_or_else(|| {
        CmdError::click(format!(
            "could not fetch registry from {}",
            store.location()
        ))
    })?;
    let value: Value = serde_json::from_str(&text)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// `stado registry self [--name-only]` — which registry target is this
/// machine. Installers need it: a plist that hardcodes a name the registry
/// does not carry produces a daemon that starts, fails its identity lookup
/// and exits, on every respawn, forever.
pub async fn self_target(name_only: bool) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = fetch_registry().await?;
    let found = registry
        .lookup_self(&hostname)
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "host {hostname} is not in {}",
                targets::registry_location()
            ))
        })?;
    if name_only {
        println!("{}", found.name);
    } else {
        println!("{}\t{}\t{}", found.name, found.kind, hostname);
    }
    Ok(())
}

/// `stado registry host add HOST --ssh DEST [--kind local]` — onboard a
/// machine into the canonical registry.
///
/// Refuses a name the registry already declares, and runs the exact
/// validation [`push`] runs BEFORE anything is written, so a colliding
/// hostname alias or an ssh destination with no host is rejected with the
/// registry-v2 contract's own message instead of landing in the store.
pub async fn host_add(host: &str, ssh: &str, kind: &str) -> Result<(), CmdError> {
    let name = targets::normalize_hostname(host);
    if name.is_empty() {
        return Err(CmdError::click("HOST must not be empty"));
    }
    if ssh.trim().is_empty() {
        return Err(CmdError::click("--ssh must not be empty"));
    }
    let location = targets::registry_location();
    let mut document = fetch_document().await?;
    let entries = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let duplicate = entries.iter().find(|entry| {
        entry
            .get("name")
            .and_then(Value::as_str)
            .map(targets::normalize_hostname)
            .as_deref()
            == Some(name.as_str())
    });
    if let Some(entry) = duplicate {
        let declared_kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(CmdError::click(format!(
            "{name} is already declared in {location} (kind={declared_kind}); \
             refusing to add a duplicate"
        )));
    }
    entries.push(json!({
        "name": name,
        "kind": kind,
        "ssh": ssh,
        "notes": "onboarded by `stado registry host add`",
    }));
    let generation = push_document(&document).await?;
    println!("added {name} (kind={kind}, ssh={ssh}) -> {location} generation={generation}");
    Ok(())
}

// ---------------------------------------------------------------------------
// live host state: beacons + capacity broadcasts
// ---------------------------------------------------------------------------

/// A beacon older than the fleet's liveness window is a divergence, not
/// jitter: the beacon republishes on the same cadence as the capacity
/// broadcast (`constants::CAPACITY_HEARTBEAT_INTERVAL_S` seconds — the
/// LaunchAgent `StartInterval` rendered by
/// `deploy/install_macos_coordinator.sh`, and the systemd unit in
/// `deploy/host-health-beacon.service`), so
/// [`capacity::CAPACITY_STALE_SECONDS`] is the same missed-publications
/// window `queue::capacity` already applies to the other liveness signal.
/// One window, both signals.
fn stale_after_seconds() -> i64 {
    capacity::CAPACITY_STALE_SECONDS as i64
}

/// One `host_health/<slug>.json` object.
struct Beacon {
    /// Store-relative object name.
    path: String,
    /// Object mtime: the authority for age, since the body's `reported_at`
    /// is stamped by the reporting host's own clock.
    updated: Option<DateTime<Utc>>,
    /// Parsed beacon body; `None` when the object is unparsable or is not
    /// a JSON object.
    body: Option<Map<String, Value>>,
}

impl Beacon {
    /// `reported_at` as the host stamped it.
    fn reported_at(&self) -> Option<&str> {
        self.body.as_ref()?.get("reported_at")?.as_str()
    }

    /// When this beacon was last known good: the object mtime, falling
    /// back to the host's own `reported_at` for backends that carry no
    /// mtime.
    fn observed_at(&self) -> Option<DateTime<Utc>> {
        self.updated.or_else(|| {
            DateTime::parse_from_rfc3339(self.reported_at()?)
                .ok()
                .map(|ts| ts.with_timezone(&Utc))
        })
    }

    /// State of one unit in the beacon's `units` map. Values are either
    /// `{"state": ...}` objects or a bare string, exactly as
    /// `monitor::host_health::format_host_health` reads them.
    fn unit_state(&self, unit: &str) -> Option<String> {
        let value = self.body.as_ref()?.get("units")?.as_object()?.get(unit)?;
        match value {
            Value::Object(state) => Some(
                state
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            Value::String(state) => Some(state.clone()),
            other => Some(other.to_string()),
        }
    }
}

/// Every beacon in the store, keyed by slug — the `<slug>.json` stem that
/// `monitor::host_health::beacon_slugs` resolves targets to.
async fn load_beacons(store: &JobStorage) -> Result<BTreeMap<String, Beacon>, CmdError> {
    let prefix = format!("{}/", host_health::HEALTH_PREFIX);
    let mut beacons = BTreeMap::new();
    for blob in store.list_blobs_with_meta(&prefix).await? {
        let Some(slug) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|stem| stem.strip_suffix(".json"))
        else {
            continue;
        };
        let body = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            });
        beacons.insert(
            slug.to_string(),
            Beacon {
                path: blob.name.clone(),
                updated: blob.updated,
                body,
            },
        );
    }
    Ok(beacons)
}

/// The beacon a target resolves to, by the same slug rule
/// `monitor::host_health::load_host_health` resolves forward.
fn beacon_for<'a>(
    target: &ComputeTarget,
    beacons: &'a BTreeMap<String, Beacon>,
) -> Option<&'a Beacon> {
    host_health::beacon_slugs(target, &target.name)
        .into_iter()
        .find_map(|slug| beacons.get(&slug))
}

/// One service a registry target declares it manages.
struct DeclaredUnit {
    /// Operator-facing service name.
    name: String,
    /// The identifier the beacon reports it under: the launchd label on
    /// macOS, the systemd unit on Linux.
    id: String,
}

/// Services declared on a target by `stado service adopt|deploy`
/// (`deploy/service.rs`), which writes a per-target `services` array. The
/// key is unknown to [`ComputeTarget`], so it round-trips through
/// [`ComputeTarget::extra`]; a target that declares none is checked for
/// liveness only, never for units.
fn declared_units(target: &ComputeTarget) -> Vec<DeclaredUnit> {
    let Some(services) = target.extra.get("services").and_then(Value::as_array) else {
        return Vec::new();
    };
    services
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
            };
            let id = text("label")
                .or_else(|| text("unit"))
                .or_else(|| text("name"))?;
            Some(DeclaredUnit {
                name: text("name").unwrap_or(id).to_string(),
                id: id.to_string(),
            })
        })
        .collect()
}

/// The canonical registry, or the reason it could not be READ. An
/// unreachable store is an error here, never an empty registry: `doctor`
/// reporting "every host is unmanaged" because the store was down is the
/// exact confusion `targets::RegistryFetchError` exists to prevent.
async fn fetch_registry() -> Result<Registry, CmdError> {
    targets::fetch_registry_remote()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))
}

// ---------------------------------------------------------------------------
// registry doctor
// ---------------------------------------------------------------------------

/// One way the registry and the live fleet disagree.
struct Finding {
    /// Stable machine-readable category.
    kind: &'static str,
    /// Target, host slug or consumer the finding is about.
    subject: String,
    /// What specifically disagrees.
    detail: String,
}

impl Finding {
    fn new(kind: &'static str, subject: impl AsRef<str>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            subject: subject.as_ref().to_string(),
            detail: detail.into(),
        }
    }

    fn to_json(&self) -> Value {
        json!({"finding": self.kind, "subject": self.subject, "detail": self.detail})
    }
}

/// `stado registry doctor [--json]` — diff registry declarations against
/// live host state: hosts with no heartbeat, stale beacons, missing
/// plists, unmanaged agents.
///
/// Exits non-zero on any divergence, naming each one, so it drops straight
/// into a cron or a CI gate. Liveness comes from the beacons and the
/// capacity broadcasts, never ssh: the whole command is one prefix listing
/// plus the bodies it finds.
#[allow(clippy::too_many_lines)]
pub async fn doctor(as_json: bool) -> Result<(), CmdError> {
    let registry = fetch_registry().await?;
    let store = JobStorage::new().await?;
    let beacons = load_beacons(&store).await?;
    let consumers = capacity::read_consumer_capacity(&store).await?;
    let now = Utc::now();

    let mut findings: Vec<Finding> = Vec::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();

    for target in &registry.targets {
        let slugs = host_health::beacon_slugs(target, &target.name);
        claimed.extend(slugs.iter().cloned());
        // Only kind=local declares a machine that runs a beacon; "gcp" and
        // "vast" targets are dispatcher pools, not boxes.
        if !target.is_provider(crate::capabilities::ProviderId::Local) {
            continue;
        }
        let Some(beacon) = slugs.iter().find_map(|slug| beacons.get(slug)) else {
            findings.push(Finding::new(
                "no-heartbeat",
                &target.name,
                format!(
                    "declared kind=local but no beacon exists; checked {}/{{{}}}.json",
                    host_health::HEALTH_PREFIX,
                    slugs.join(",")
                ),
            ));
            continue;
        };
        match beacon.observed_at() {
            Some(observed) => {
                let age = now - observed;
                if age.num_seconds() > stale_after_seconds() {
                    findings.push(Finding::new(
                        "stale-beacon",
                        &target.name,
                        format!(
                            "{} last updated {} ago ({}), past the {}s liveness window",
                            beacon.path,
                            human_age(age),
                            observed.to_rfc3339(),
                            stale_after_seconds()
                        ),
                    ));
                }
            }
            None => findings.push(Finding::new(
                "stale-beacon",
                &target.name,
                format!(
                    "{} carries neither an object timestamp nor reported_at",
                    beacon.path
                ),
            )),
        }
        for declared in declared_units(target) {
            match beacon.unit_state(&declared.id) {
                None => findings.push(Finding::new(
                    "missing-plist",
                    &target.name,
                    format!(
                        "registry declares service {} ({}) but {} reports no such unit",
                        declared.name, declared.id, beacon.path
                    ),
                )),
                Some(state) if state != ACTIVE_STATE => findings.push(Finding::new(
                    "unit-not-active",
                    &target.name,
                    format!(
                        "registry declares service {} ({}) but {} reports state={state}",
                        declared.name, declared.id, beacon.path
                    ),
                )),
                Some(_) => {}
            }
        }
    }

    for (slug, beacon) in &beacons {
        if !claimed.contains(slug) {
            findings.push(Finding::new(
                "unmanaged-host",
                slug,
                format!(
                    "{} is publishing beacons but no registry target claims that identity",
                    beacon.path
                ),
            ));
        }
    }

    for (consumer_id, payload) in &consumers {
        // Only local agents map one-to-one onto a registry box; a "gcp" or
        // "vast" broadcast comes from an ephemeral VM the registry
        // deliberately does not enumerate.
        if !payload
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| crate::capabilities::ProviderId::Local.matches(kind))
        {
            continue;
        }
        // consumer_id is "<kind>-<hostname>"
        // (`queue::capacity::publish_capacity`); `scheduler::makespan`
        // splits it exactly this way.
        let host = consumer_id
            .split_once('-')
            .map_or(consumer_id.as_str(), |(_, host)| host);
        let declared = registry
            .lookup_self(host)
            .map_err(|exc| CmdError::click(exc.to_string()))?
            .is_some();
        if !declared {
            findings.push(Finding::new(
                "unmanaged-agent",
                consumer_id,
                format!(
                    "broadcasting live capacity as host {host}, which no registry target declares"
                ),
            ));
        }
    }

    let location = targets::registry_location();
    if as_json {
        echo_json(&json!({
            "registry": location,
            "ok": findings.is_empty(),
            "checked": {
                "targets": registry.targets.len(),
                "beacons": beacons.len(),
                "capacity_consumers": consumers.len(),
            },
            "findings": findings.iter().map(Finding::to_json).collect::<Vec<Value>>(),
        }));
    } else if findings.is_empty() {
        println!(
            "registry {location} agrees with live host state ({} targets, {} beacons, \
             {} live consumers)",
            registry.targets.len(),
            beacons.len(),
            consumers.len()
        );
    } else {
        let rows: Vec<Vec<String>> = findings
            .iter()
            .map(|finding| {
                vec![
                    finding.kind.to_string(),
                    finding.subject.clone(),
                    finding.detail.clone(),
                ]
            })
            .collect();
        table::print(&["FINDING", "SUBJECT", "DETAIL"], &rows);
    }
    if findings.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} divergence(s) between {location} and live host state",
        findings.len()
    )))
}

// ---------------------------------------------------------------------------
// registry beacon-age
// ---------------------------------------------------------------------------

/// Sort rank, worst first. Derived `Ord` follows declaration order, so a
/// host that never reported outranks a stale one and a target that is not
/// supposed to have a beacon sinks to the bottom.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BeaconRank {
    /// kind=local with no beacon object at all.
    Missing,
    /// Has a beacon; ordered oldest-first within the rank.
    Reported,
    /// Not a machine (kind=gcp / kind=vast): no beacon is expected.
    NotExpected,
}

impl BeaconRank {
    fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Reported => "reported",
            Self::NotExpected => "not-applicable",
        }
    }
}

/// One registry host paired with the beacon that proves it is alive.
struct BeaconRow {
    name: String,
    kind: String,
    rank: BeaconRank,
    observed: Option<DateTime<Utc>>,
    reported_at: Option<String>,
    path: Option<String>,
}

/// Largest whole unit of an age, e.g. `5d` — the "has not reported in
/// days" signal at a glance. chrono performs every unit conversion, so no
/// seconds-per-day arithmetic is hand-rolled here.
///
/// Shared with `cli::host::ping`, which grades the same beacon for a
/// single host: one spelling of an age across both commands.
pub(crate) fn human_age(age: TimeDelta) -> String {
    for (amount, suffix) in [
        (age.num_days(), "d"),
        (age.num_hours(), "h"),
        (age.num_minutes(), "m"),
    ] {
        if amount.is_positive() {
            return format!("{amount}{suffix}");
        }
    }
    format!("{}s", age.num_seconds().max(i64::default()))
}

/// `stado registry beacon-age [--json]` — every registry host and its last
/// beacon, worst first.
///
/// Lists hosts with no beacon at all: a machine that silently stopped
/// reporting is exactly what this table exists to surface, and a row that
/// is absent surfaces nothing.
pub async fn beacon_age(as_json: bool) -> Result<(), CmdError> {
    let registry = fetch_registry().await?;
    let store = JobStorage::new().await?;
    let beacons = load_beacons(&store).await?;
    let now = Utc::now();

    let mut rows: Vec<BeaconRow> = registry
        .targets
        .iter()
        .map(|target| {
            let beacon = beacon_for(target, &beacons);
            let rank = if beacon.is_some() {
                BeaconRank::Reported
            } else if target.is_provider(crate::capabilities::ProviderId::Local) {
                BeaconRank::Missing
            } else {
                BeaconRank::NotExpected
            };
            BeaconRow {
                name: target.name.clone(),
                kind: target.kind.clone(),
                rank,
                observed: beacon.and_then(Beacon::observed_at),
                reported_at: beacon.and_then(Beacon::reported_at).map(str::to_string),
                path: beacon.map(|beacon| beacon.path.clone()),
            }
        })
        .collect();
    // (rank, observed) ascending: never-reported first, then the oldest
    // beacon, with the not-applicable rows last.
    rows.sort_by_key(|row| (row.rank, row.observed));

    let age_of = |row: &BeaconRow| row.observed.map(|observed| now - observed);
    if as_json {
        let hosts: Vec<Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "host": row.name,
                    "kind": row.kind,
                    "status": row.rank.label(),
                    "beacon": row.path,
                    "observed_at": row.observed.map(|ts| ts.to_rfc3339()),
                    "reported_at": row.reported_at,
                    "age_seconds": age_of(row).map(|age| age.num_seconds()),
                })
            })
            .collect();
        echo_json(&json!({"registry": targets::registry_location(), "hosts": hosts}));
        return Ok(());
    }

    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let age = match (row.rank, age_of(row)) {
                (BeaconRank::Missing, _) => "never".to_string(),
                (BeaconRank::NotExpected, None) => "-".to_string(),
                (_, Some(age)) => human_age(age),
                (_, None) => "unknown".to_string(),
            };
            vec![
                row.name.clone(),
                row.kind.clone(),
                age,
                row.observed
                    .map_or_else(|| "-".to_string(), |ts| ts.to_rfc3339()),
                row.reported_at.clone().unwrap_or_else(|| "-".to_string()),
                row.path.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    table::print(
        &["HOST", "KIND", "AGE", "OBSERVED", "REPORTED_AT", "BEACON"],
        &table_rows,
    );
    Ok(())
}

/// Python `click.echo(json.dumps(payload, indent=2, sort_keys=True))`.
fn echo_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(exc) => eprintln!("could not render json: {exc}"),
    }
}
