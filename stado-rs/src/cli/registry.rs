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

use crate::capabilities::{Consumer, DeclarationSurface, DeclaredField, SiblingCondition};
use crate::deploy::service;
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
/// the next, and gained a `service_directory` block no checkout in the tree
/// modelled at the time — divergent builds writing the same object, each
/// erasing what it could not name. `targets::Registry` now keeps unmodelled
/// top-level keys in `extra`, and `fetch_document` hands read-modify-write
/// callers the raw document; this is the backstop for a payload that came
/// from neither.
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

/// The service directory's own publication counter, when the document carries
/// one.
///
/// `ServiceDirectory::generation` is what a consumer compares against the copy
/// it cached, and `ServiceDirectoryError::Stale` is the answer it gets when its
/// copy is older. That check only means something if the number never goes
/// backwards at the authority.
fn service_directory_generation(text: &str) -> Option<u64> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("service_directory")?
        .get("generation")?
        .as_u64()
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
        if let Some(blob) = current.as_ref() {
            // Same accident as the deleted-key guard above, one level in: the
            // whole document is replaced, so a writer holding an older copy
            // publishes its older directory over a newer one and every
            // consumer's staleness check silently starts agreeing with it.
            // Observed on 2026-08-12, when the directory went from generation
            // 10 back to 5 and two corrected endpoints reverted with it.
            if let (Some(before), Some(after)) = (
                service_directory_generation(&blob.content),
                service_directory_generation(payload),
            ) {
                if after < before {
                    return Err(CmdError::click(format!(
                        "registry upload refused: its service directory is generation \
                         {after} and the registry already carries {before}. The counter \
                         consumers use to detect a stale directory would go backwards, \
                         so every cached copy older than {before} would start looking \
                         current. Re-pull, re-apply the edit, and push again; pass \
                         --force only if publishing the older directory is the intent."
                    )));
                }
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
/// Read-modify-write callers work on the raw document rather than on
/// [`Registry`] because an edit here is a surgical key change, and the raw
/// value is the shortest path to one. [`Registry`] is no longer lossy —
/// unmodelled top-level keys round-trip through `Registry::extra` and
/// `Registry::to_document` writes them back — so either route preserves the
/// document; this one simply does not re-serialize the parts it never
/// touched.
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
    let registry = read_registry().await?;
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
pub async fn host_add(
    host: &str,
    ssh: &str,
    kind: &str,
    release_platform: &str,
) -> Result<(), CmdError> {
    let name = targets::normalize_hostname(host);
    if name.is_empty() {
        return Err(CmdError::click("HOST must not be empty"));
    }
    if ssh.trim().is_empty() {
        return Err(CmdError::click("--ssh must not be empty"));
    }
    let location = targets::registry_location();
    let (mut document, expected_generation) = fetch_versioned_document().await?;
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
        "release_platform": release_platform,
        "notes": "onboarded by `stado registry host add`",
    }));
    let generation = push_document_if(&document, &expected_generation).await?;
    println!(
        "added {name} (kind={kind}, release_platform={release_platform}, ssh={ssh}) -> \
         {location} generation={generation}"
    );
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

/// The canonical registry for a read-only command, the last-known-good copy
/// when the authority does not answer, or the reason neither could be READ.
///
/// Never an empty registry: `doctor` reporting "every host is unmanaged"
/// because the store was down is the exact confusion
/// `targets::RegistryFetchError` exists to prevent. Never a silent copy
/// either — the copy's age goes to stderr in one sentence, because a
/// diagnostic that dies with the thing it diagnoses is worthless, and a
/// diagnostic that answers from a copy without saying so is worse.
pub(crate) async fn read_registry() -> Result<Registry, CmdError> {
    let (registry, notice) = targets::fetch_registry_or_last_good()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if let Some(notice) = notice {
        targets::report_registry_notice(&notice);
    }
    Ok(registry)
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
    /// Unit label this finding is about, when it is about one.
    ///
    /// Two rows for one cause is noise. A unit that is missing because its host
    /// cannot satisfy what the unit needs produces both a `capability-unsatisfied`
    /// and a `missing-plist`; the second is the symptom of the first, and
    /// [`doctor`] drops it so the row that survives names the cause.
    unit: Option<String>,
}

impl Finding {
    fn new(kind: &'static str, subject: impl AsRef<str>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            subject: subject.as_ref().to_string(),
            detail: detail.into(),
            unit: None,
        }
    }

    /// Name the unit this finding is about, so one cause cannot be reported twice.
    fn about(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }

    fn to_json(&self) -> Value {
        json!({"finding": self.kind, "subject": self.subject, "detail": self.detail})
    }
}

/// Object prefix the per-host capability measurements live under, alongside
/// `host_health/` and read through the same object API
/// (`stado://<namespace>/host_capabilities/<registry-target-name>.json`).
const CAPABILITIES_PREFIX: &str = "host_capabilities";

/// The only measurement schema this build understands. A document carrying
/// anything else is reported rather than guessed at: a requirement checked
/// against a shape nobody agreed on is worse than an unchecked one.
const CAPABILITIES_SCHEMA: &str = "wisent.host-capabilities.v1";

/// One measured capability: the answer, and the measurement that produced it.
pub(crate) struct MeasuredCapability {
    pub(crate) value: bool,
    pub(crate) detail: String,
}

/// One `host_capabilities/<target>.json` object.
pub(crate) struct Measurement {
    /// Store-relative object name, so a finding names what to go and read.
    path: String,
    schema: String,
    /// The host's own stamp, falling back to the object mtime the same way
    /// [`Beacon::observed_at`] does.
    pub(crate) measured_at: Option<DateTime<Utc>>,
    pub(crate) capabilities: BTreeMap<String, MeasuredCapability>,
}

/// Every capability measurement in the store, keyed by the registry target name
/// the object is published under.
///
/// One prefix listing plus the bodies it finds, exactly like [`load_beacons`]:
/// the two signals are published the same way and are read the same way.
pub(crate) async fn load_capability_measurements(
    store: &JobStorage,
) -> Result<BTreeMap<String, Measurement>, crate::queue::StorageError> {
    let prefix = format!("{CAPABILITIES_PREFIX}/");
    let mut measurements = BTreeMap::new();
    for blob in store.list_blobs_with_meta(&prefix).await? {
        let Some(target) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|stem| stem.strip_suffix(".json"))
        else {
            continue;
        };
        let Some(body) = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let capabilities = body
            .get("capabilities")
            .and_then(Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(id, entry)| {
                        (
                            id.clone(),
                            MeasuredCapability {
                                value: entry.get("value").and_then(Value::as_bool) == Some(true),
                                detail: entry
                                    .get("detail")
                                    .and_then(Value::as_str)
                                    .unwrap_or("no detail recorded")
                                    .to_string(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        measurements.insert(
            target.to_string(),
            Measurement {
                path: blob.name.clone(),
                schema: body
                    .get("schema")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                measured_at: body
                    .get("measured_at")
                    .and_then(Value::as_str)
                    .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
                    .map(|stamp| stamp.with_timezone(&Utc))
                    .or(blob.updated),
                capabilities,
            },
        );
    }
    Ok(measurements)
}

/// Object prefix the published job requirement declarations live under, read
/// through the same object API as the beacons and the measurements.
const REQUIREMENTS_PREFIX: &str = "job_requirements";

/// The only requirement schema this build understands.
const REQUIREMENTS_SCHEMA: &str = "wisent.trajectory-requirements.v1";

/// How long a published requirement declaration is believed.
///
/// Deliberately not the beacon window: a requirement is republished when the job
/// changes, not on a heartbeat, so judging it in minutes would mark every
/// declaration stale within one and teach operators to skip the row. A week is
/// longer than the fleet goes between Weles releases, so an object older than
/// that is a declaration nobody is republishing — which is the failure this
/// window exists to catch, an object that no longer matches the repository file
/// it is supposed to be a copy of.
fn requirements_stale_after_seconds() -> i64 {
    TimeDelta::weeks(1).num_seconds()
}

/// One declared service and the job it runs.
struct RequirementClaim {
    /// Service name as the registry spells it.
    unit: String,
    /// The identifier the beacon reports the unit under, so a `missing-plist` row
    /// for the same unit can be recognised as this finding's symptom.
    label: String,
    /// Trajectory id the service entry names, e.g. `kimi/login`.
    trajectory: String,
}

/// Every published requirement declaration, resolved to what each trajectory
/// needs.
struct Declarations {
    /// Trajectory id -> the object that declares it and the capability ids it
    /// names.
    needs: BTreeMap<String, (String, Vec<String>)>,
    /// Objects found under the prefix, so a finding can say what was consulted.
    objects: Vec<String>,
    /// Objects this build would not read, and why. A service pointing into one is
    /// reported rather than silently treated as satisfied.
    refused: Vec<String>,
}

impl Declarations {
    /// What was consulted, for a finding that has to explain an absence.
    fn consulted(&self) -> String {
        let read = if self.objects.is_empty() {
            format!("no object exists under {REQUIREMENTS_PREFIX}/")
        } else {
            format!("read {}", self.objects.join(", "))
        };
        if self.refused.is_empty() {
            read
        } else {
            format!("{read}; refused {}", self.refused.join("; "))
        }
    }
}

/// Every requirement declaration in the store.
///
/// The declaration is the job author's document, published verbatim
/// (`stado://<namespace>/job_requirements/weles-trajectories.json` carries the
/// bytes of `weles/scripts/trajectories/requirements.json`). The registry names
/// which job a service runs and nothing more, so this is the one place a
/// capability list for a job exists — a copy in the registry would be the second
/// source of truth that `unread-declaration` and this whole command exist to
/// prevent.
async fn load_job_requirements(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<Declarations, crate::queue::StorageError> {
    let prefix = format!("{REQUIREMENTS_PREFIX}/");
    let mut declarations = Declarations {
        needs: BTreeMap::new(),
        objects: Vec::new(),
        refused: Vec::new(),
    };
    for blob in store.list_blobs_with_meta(&prefix).await? {
        if !blob.name.ends_with(".json") {
            continue;
        }
        declarations.objects.push(blob.name.clone());
        let Some(body) = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            declarations
                .refused
                .push(format!("{} is not readable JSON", blob.name));
            continue;
        };
        let schema = body
            .get("schema")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if schema != REQUIREMENTS_SCHEMA {
            declarations.refused.push(format!(
                "{} carries schema {schema:?} rather than {REQUIREMENTS_SCHEMA}",
                blob.name
            ));
            continue;
        }
        if let Some(published) = blob.updated {
            let age = now - published;
            if age.num_seconds() > requirements_stale_after_seconds() {
                declarations.refused.push(format!(
                    "{} was published {} ago ({}), past the {}s republication window",
                    blob.name,
                    human_age(age),
                    published.to_rfc3339(),
                    requirements_stale_after_seconds()
                ));
                continue;
            }
        }
        let Some(trajectories) = body.get("trajectories").and_then(Value::as_object) else {
            declarations
                .refused
                .push(format!("{} carries no trajectories map", blob.name));
            continue;
        };
        for (trajectory, value) in trajectories {
            // A non-string entry is dropped rather than guessed at: a garbled
            // requirement must not be able to pass as a satisfied one.
            let capabilities = value
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            declarations
                .needs
                .insert(trajectory.clone(), (blob.name.clone(), capabilities));
        }
    }
    Ok(declarations)
}

/// Which job each service declared on this target runs.
///
/// The registry's whole role in this join is the identifier: `trajectory` on the
/// service entry, never a capability list. A service that names no trajectory
/// declares no requirement, so every registry written before this field existed
/// stays clean.
fn declared_trajectories(target: &ComputeTarget) -> Vec<RequirementClaim> {
    let Some(services) = target.extra.get("services").and_then(Value::as_array) else {
        return Vec::new();
    };
    services
        .iter()
        .filter_map(|entry| {
            let trajectory = entry
                .get("trajectory")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            };
            let unit = text("name").or_else(|| text("label"));
            Some(RequirementClaim {
                unit: unit.unwrap_or("(unnamed service)").to_string(),
                // The beacon reports a unit under its launchd label or systemd
                // unit, and that is the key the missing-plist row is filed
                // under, so it is resolved here exactly as `declared_units`
                // resolves it.
                label: text("label")
                    .or_else(|| text("unit"))
                    .or(unit)
                    .unwrap_or_default()
                    .to_string(),
                trajectory: trajectory.to_string(),
            })
        })
        .collect()
}

/// What stops one host's last measurement from satisfying a capability list, one
/// clause per reason, empty when nothing does.
///
/// The single place that answers "can this host run this job", so the finding
/// about a host that declares the job and the finding about a job no host
/// declares cannot answer it differently. A missing, mis-schema'd or stale
/// measurement disqualifies the host in one clause rather than once per
/// capability: the operator's next action is the same however many ids were
/// named, and it is to go and measure the host.
fn measurement_gaps(
    target: &str,
    capabilities: &[String],
    measurement: Option<&Measurement>,
    now: DateTime<Utc>,
) -> Vec<String> {
    // A job that needs nothing of the host is satisfied by every host, measured
    // or not: `codex/reauth` declares exactly that.
    if capabilities.is_empty() {
        return Vec::new();
    }
    let Some(measurement) = measurement else {
        return vec![format!(
            "{CAPABILITIES_PREFIX}/{target}.json does not exist: nothing has measured this host"
        )];
    };
    if measurement.schema != CAPABILITIES_SCHEMA {
        return vec![format!(
            "{} carries schema {:?} rather than {CAPABILITIES_SCHEMA}",
            measurement.path, measurement.schema
        )];
    }
    match measurement.measured_at {
        None => {
            return vec![format!(
                "{} carries neither measured_at nor an object timestamp, so its age cannot be \
                 judged",
                measurement.path
            )]
        }
        Some(measured) => {
            let age = now - measured;
            if age.num_seconds() > stale_after_seconds() {
                return vec![format!(
                    "{} was measured {} ago ({}), past the {}s liveness window",
                    measurement.path,
                    human_age(age),
                    measured.to_rfc3339(),
                    stale_after_seconds()
                )];
            }
        }
    }
    capabilities
        .iter()
        .filter_map(
            |capability| match measurement.capabilities.get(capability) {
                None => Some(format!(
                    "{} does not measure {capability}",
                    measurement.path
                )),
                Some(measured) if !measured.value => {
                    Some(format!("{capability} measured false: {}", measured.detail))
                }
                Some(_) => None,
            },
        )
        .collect()
}

/// Every hop of the join that fails for one declared service, one sentence each:
/// declared service -> trajectory id -> published requirement -> measured
/// capability. Empty means the host is measured able to run what it is declared to
/// run.
fn unmet_requirements(
    target: &str,
    claim: &RequirementClaim,
    declarations: &Declarations,
    measurement: Option<&Measurement>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let unit = &claim.unit;
    let trajectory = &claim.trajectory;
    let Some((source, capabilities)) = declarations.needs.get(trajectory) else {
        return vec![format!(
            "{unit} runs trajectory {trajectory}, and no published declaration names it: {}",
            declarations.consulted()
        )];
    };
    let needs = capabilities.join(", ");
    measurement_gaps(target, capabilities, measurement, now)
        .into_iter()
        .map(|gap| {
            format!("{unit} runs {trajectory}, which {source} says requires {needs}, and {gap}")
        })
        .collect()
}

/// Jobs in the published roster that nothing runs and no host could.
///
/// A declared service entry is the RESULT of a placement that succeeded, so a
/// trajectory no target declares is a job waiting for a host. That is only worth
/// reporting when no candidate can take it: while some measured host satisfies the
/// requirement, placement has an answer and the absence is a step not yet taken
/// rather than a contradiction. The row names each candidate and the measurement
/// that disqualified it, so it says what would have to change, and it disappears by
/// itself the moment a capable host exists and the unit is adopted where it runs.
fn unplaced_jobs(
    registry: &Registry,
    declarations: &Declarations,
    measurements: &BTreeMap<String, Measurement>,
    placed: &BTreeSet<&str>,
    now: DateTime<Utc>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (trajectory, (source, capabilities)) in &declarations.needs {
        if capabilities.is_empty() || placed.contains(trajectory.as_str()) {
            continue;
        }
        let mut disqualified = Vec::new();
        for target in &registry.targets {
            // Only kind=local names a machine that can hold a session and run a
            // browser; "gcp" and "vast" targets are dispatcher pools.
            if !target.is_provider(crate::capabilities::ProviderId::Local) {
                continue;
            }
            let gaps = measurement_gaps(
                &target.name,
                capabilities,
                measurements.get(&target.name),
                now,
            );
            if gaps.is_empty() {
                disqualified.clear();
                break;
            }
            disqualified.push(format!("{}: {}", target.name, gaps.join(", ")));
        }
        if !disqualified.is_empty() {
            findings.push(Finding::new(
                "job-unplaced",
                trajectory,
                format!(
                    "{source} says it requires {}, no registry target declares a service that \
                     runs it, and no host can take it — {}",
                    capabilities.join(", "),
                    disqualified.join("; ")
                ),
            ));
        }
    }
    findings
}

/// The value at a dotted path, or `None` when any segment is absent.
fn value_at<'a>(root: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted
        .split('.')
        .try_fold(root, |value, key| value.get(key))
}

/// One line of JSON for a declared value, so a finding shows what was written
/// rather than only where.
fn rendered(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// Why a declared value never reaches behaviour, or `None` when it does.
///
/// The catalog decides, in one place, for both surfaces: a fleet reader whose
/// reachability condition holds is read, and everything else is a declaration an
/// operator wrote for nobody.
fn unread_reason(field: &DeclaredField, sibling: Option<String>) -> Option<String> {
    match field.consumer {
        Consumer::Fleet(reader) => {
            let condition = field.reached_when?;
            let observed = sibling.unwrap_or_else(|| "(absent)".to_string());
            if observed.starts_with(condition.value_prefix) {
                return None;
            }
            Some(format!(
                "its only reader {reader} runs when {} starts with {:?}, and that key is {observed}",
                condition.path, condition.value_prefix
            ))
        }
        Consumer::OperatorCopy {
            command,
            destination,
        } => Some(format!(
            "no fleet process reads it: only `{command}` copies it to {destination}, and only when \
             an operator runs that command"
        )),
        Consumer::Unread => Some("no code path in this build reads it".to_string()),
    }
}

/// A sibling value rendered for a finding: strings bare, everything else as JSON,
/// so a URL reads as a URL and a number still reads unambiguously.
fn sibling_value(root: &Value, condition: SiblingCondition) -> Option<String> {
    value_at(root, condition.path).map(|value| {
        value
            .as_str()
            .map_or_else(|| rendered(value), str::to_string)
    })
}

/// Registry fields on one target that no consumer reads, from both halves of the
/// rule.
///
/// The derived half needs no catalog: a key [`ComputeTarget`] does not model
/// lands in `extra`, which is by construction the set of keys no typed reader in
/// this build can name, so a field added tomorrow with no reader fails without
/// anybody remembering to declare it. The catalogued half covers what the derived
/// half cannot see — a path inside a block this build does model, where the
/// deserializer accepting a value proves nothing about anyone acting on it.
fn unread_declarations(target: &ComputeTarget, entry: &Value) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (key, value) in &target.extra {
        match crate::capabilities::declared_field(DeclarationSurface::RegistryTarget, key) {
            Some(field) => {
                if let Some(reason) = unread_reason(
                    field,
                    field.reached_when.and_then(|c| sibling_value(entry, c)),
                ) {
                    findings.push(Finding::new(
                        "unread-declaration",
                        &target.name,
                        format!(
                            "{} {key} is declared as {} but {reason}",
                            field.surface.label(),
                            rendered(value)
                        ),
                    ));
                }
            }
            None => findings.push(Finding::new(
                "unread-declaration",
                &target.name,
                format!(
                    "registry target key {key} is declared as {} and is neither modelled by \
                     ComputeTarget nor catalogued in capabilities::DECLARED_FIELDS, so no reader \
                     in this build can consult it",
                    rendered(value)
                ),
            )),
        }
    }
    // Dotted paths only: a top-level catalogued key that this build does not
    // model was already answered by the loop above, and answering it twice would
    // report one defect as two.
    for field in crate::capabilities::DECLARED_FIELDS {
        if field.surface != DeclarationSurface::RegistryTarget || !field.path.contains('.') {
            continue;
        }
        let Some(value) = value_at(entry, field.path) else {
            continue;
        };
        let Some(reason) = unread_reason(
            field,
            field.reached_when.and_then(|c| sibling_value(entry, c)),
        ) else {
            continue;
        };
        findings.push(Finding::new(
            "unread-declaration",
            &target.name,
            format!(
                "{} {} is declared as {} but {reason}",
                field.surface.label(),
                field.path,
                rendered(value)
            ),
        ));
    }
    findings
}

/// Configuration keys this deployment carries that no reader on it can consult.
///
/// The document is the one this process would honour, so the answer is about the
/// deployment actually running rather than about the schema. Reading it cannot
/// fail here: every caller reaches `doctor` through a storage handle that already
/// loaded and parsed the same file.
fn unread_configuration() -> Vec<Finding> {
    let subject = crate::config_file::config_path()
        .ok()
        .flatten()
        .map_or_else(
            || "stado config".to_string(),
            |path| path.display().to_string(),
        );
    let mut findings = Vec::new();
    for field in crate::capabilities::DECLARED_FIELDS {
        if field.surface != DeclarationSurface::Configuration {
            continue;
        }
        let Some(value) = crate::config_file::get(field.path).filter(|value| !value.is_null())
        else {
            continue;
        };
        let sibling = field
            .reached_when
            .and_then(|condition| crate::config_file::get(condition.path))
            .map(|value| {
                value
                    .as_str()
                    .map_or_else(|| rendered(&value), str::to_string)
            });
        if let Some(reason) = unread_reason(field, sibling) {
            findings.push(Finding::new(
                "unread-declaration",
                &subject,
                format!(
                    "{} {} is declared as {} but {reason}",
                    field.surface.label(),
                    field.path,
                    rendered(&value)
                ),
            ));
        }
    }
    findings
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
    let registry = read_registry().await?;
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
        // The document against itself, before any beacon is consulted: a
        // launchd domain the host cannot have is wrong whether or not the
        // host is answering, and it is the reason its beacon will never
        // report the unit.
        for misdeclared in service::misdeclared_domains(target) {
            let unit = misdeclared.unit.clone();
            findings.push(
                Finding::new(
                    "misdeclared-domain",
                    &target.name,
                    misdeclared.sentence(),
                )
                .about(unit),
            );
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
                None => findings.push(
                    Finding::new(
                        "missing-plist",
                        &target.name,
                        format!(
                            "registry declares service {} ({}) but {} reports no such unit",
                            declared.name, declared.id, beacon.path
                        ),
                    )
                    .about(declared.id.as_str()),
                ),
                Some(state) if state != ACTIVE_STATE => findings.push(
                    Finding::new(
                        "unit-not-active",
                        &target.name,
                        format!(
                            "registry declares service {} ({}) but {} reports state={state}",
                            declared.name, declared.id, beacon.path
                        ),
                    )
                    .about(declared.id.as_str()),
                ),
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

    // Three declaration checks the beacons cannot answer. They compare the
    // document with itself and with the last measurement rather than with a
    // heartbeat, so they run for every target, including the dispatcher pools
    // the liveness checks above skip.
    //
    // The raw document, not the typed `Registry`: `service_directory` and
    // `service_resolver` are raw-JSON blocks whose model lives in
    // `service_resolution`, and the unread-declaration check is about exactly the
    // keys no model in this build names.
    let document = fetch_document().await?;
    let entries: BTreeMap<&str, &Value> = document
        .get("targets")
        .and_then(Value::as_array)
        .map(|targets| {
            targets
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .map(|name| (name, entry))
                })
                .collect()
        })
        .unwrap_or_default();
    for loop_back in
        crate::service_resolution::self_referencing_endpoints(&document).map_err(CmdError::click)?
    {
        findings.push(Finding::new(
            "self-referencing-endpoint",
            &loop_back.target,
            format!(
                "service_directory.services.{}.{}.{} is {}, which is {} on that same target: the \
                 adapter would proxy to itself",
                loop_back.service,
                loop_back.map,
                loop_back.target,
                loop_back.address,
                loop_back.adapter
            ),
        ));
    }

    // Which declared service runs which job. The published roster is read whether
    // or not anything declares one, because a job the roster names and no target
    // declares is itself a finding: a declared service entry is the result of a
    // placement that succeeded, so a job with no entry anywhere is a job waiting
    // for a host that can take it.
    let claims: Vec<(&ComputeTarget, RequirementClaim)> = registry
        .targets
        .iter()
        .flat_map(|target| {
            declared_trajectories(target)
                .into_iter()
                .map(move |claim| (target, claim))
        })
        .collect();
    let mut measured_hosts = usize::default();
    let mut roster = usize::default();
    // An unreadable prefix is not an absent object, and reporting it as one would
    // say a host cannot do something when the truth is that nobody here may look.
    // Both reads share that reasoning, so both report the store's own words.
    match (
        load_job_requirements(&store, now).await,
        load_capability_measurements(&store).await,
    ) {
        (Ok(declarations), Ok(measurements)) => {
            measured_hosts = measurements.len();
            roster = declarations.needs.len();
            for (target, claim) in &claims {
                for reason in unmet_requirements(
                    &target.name,
                    claim,
                    &declarations,
                    measurements.get(&target.name),
                    now,
                ) {
                    findings.push(
                        Finding::new("capability-unsatisfied", &target.name, reason)
                            .about(claim.label.as_str()),
                    );
                }
            }
            let placed: BTreeSet<&str> = claims
                .iter()
                .map(|(_, claim)| claim.trajectory.as_str())
                .collect();
            findings.extend(unplaced_jobs(
                &registry,
                &declarations,
                &measurements,
                &placed,
                now,
            ));
        }
        // One row, not one per claim: the cause is a store that will not answer,
        // and it is the same cause for every job.
        (declarations, measurements) => {
            let refused = [
                declarations
                    .err()
                    .map(|exc| format!("{REQUIREMENTS_PREFIX}/ could not be read: {exc}")),
                measurements
                    .err()
                    .map(|exc| format!("{CAPABILITIES_PREFIX}/ could not be read: {exc}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<String>>()
            .join("; ");
            findings.push(Finding::new(
                "capability-unsatisfied",
                format!("{REQUIREMENTS_PREFIX}/"),
                format!(
                    "{} declared trajectory claim(s) cannot be judged and no job can be placed: \
                     {refused}",
                    claims.len()
                ),
            ));
        }
    }

    for target in &registry.targets {
        let empty = Value::Null;
        let entry = entries.get(target.name.as_str()).copied().unwrap_or(&empty);
        findings.extend(unread_declarations(target, entry));
    }
    findings.extend(unread_configuration());

    // One cause, one row. A unit the beacon does not report on a host that cannot
    // satisfy what the unit needs is already reported as `capability-unsatisfied`,
    // and the `missing-plist` row for the same label is that finding's symptom:
    // installing the plist would not make the host able to run it. A unit
    // declared in a domain its host cannot have is the same relationship —
    // `misdeclared-domain` is why nothing loads it and why no beacon reports
    // it, and installing the plist where it is declared would change neither.
    let caused: Vec<(String, String)> = findings
        .iter()
        .filter(|finding| {
            matches!(finding.kind, "capability-unsatisfied" | "misdeclared-domain")
        })
        .filter_map(|finding| {
            finding
                .unit
                .clone()
                .map(|unit| (finding.subject.clone(), unit))
        })
        .collect();
    findings.retain(|finding| {
        if finding.kind != "missing-plist" {
            return true;
        }
        let Some(unit) = finding.unit.as_deref() else {
            return true;
        };
        !caused
            .iter()
            .any(|(subject, symptom)| subject == &finding.subject && symptom == unit)
    });

    let location = targets::registry_location();
    if as_json {
        echo_json(&json!({
            "registry": location,
            "ok": findings.is_empty(),
            "checked": {
                "targets": registry.targets.len(),
                "beacons": beacons.len(),
                "capacity_consumers": consumers.len(),
                "requirement_claims": claims.len(),
                "declared_trajectories": roster,
                "capability_measurements": measured_hosts,
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
    let registry = read_registry().await?;
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
