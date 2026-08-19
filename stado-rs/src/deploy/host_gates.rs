//! `stado host gates HOST` — the one payload that answers "why is this host
//! claiming nothing".
//!
//! NO Python original. The incident it exists for: the Mac mini's data volume
//! sat at roughly 2 GiB free against a 55 GiB registry policy. Its queue agent
//! computes [`disk_cleanup::disk_pressure_unresolved`] every tick, publishes
//! that word in its capacity broadcast, and fails admission CLOSED while it is
//! true — zero free slots, no claim, deliberately
//! ([`crate::providers::local::agent`]). So the host claimed nothing for
//! hours, every release build queued behind it, and no command in this CLI
//! said any of it: `host disk` printed the free bytes and the policy but never
//! the admission verdict, `registry doctor` listed the host as broadcasting
//! normally, and the one fact that mattered — the agent had stopped claiming,
//! on purpose, for a reason it was republishing every tick — existed only
//! inside `capacity/<consumer>.json`, which nothing read.
//!
//! Three sources, joined here and re-derived nowhere:
//!
//! - the host's own capacity publication (`capacity/<consumer>.json`), whose
//!   `diag` words are reported VERBATIM. A blocker an operator reads here has
//!   to be greppable in the agent that published it, otherwise the CLI has
//!   invented a second vocabulary for the same condition;
//! - the registry target: its declared `slots`, and its
//!   [`crate::targets::DiskCleanupPolicy`] serialized as it stands;
//! - `df -Pk /` and the janitor's own state file, read with the exact script
//!   [`crate::deploy::host_disk`] sends, so `host gates` and `host disk` can
//!   never disagree about how much space this host has.
//!
//! Read-only, and safe against a live production host: one ssh read of one
//! `df` and one `cat`, plus one object read. Nothing restarts, nothing cycles,
//! nothing is deleted. The write half — actually getting the space back — is
//! [`crate::deploy::host_reclaim`].

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use super::host_channel;
use super::{host_disk, DeployError, Runner};
use crate::providers::local::disk_cleanup;
use crate::queue::{capacity, JobStorage};
use crate::targets::{ComputeTarget, Registry};

/// The agent's own word for "I cannot prove there is room, so I claim
/// nothing": [`disk_cleanup::disk_pressure_unresolved`], published under this
/// exact key in the capacity broadcast's `diag`.
pub const DISK_PRESSURE_UNRESOLVED: &str = "disk_pressure_unresolved";

/// The agent publishes `disk_cleanup_policy_known: false` when it has no
/// validated low watermark at all. Reported as its negation because a blocker
/// list reads as a list of things that are wrong; the underlying key, and the
/// only place the value comes from, is still the agent's.
///
/// It never appears alone: an unknown threshold also makes
/// [`DISK_PRESSURE_UNRESOLVED`] true by that function's own truth table. It is
/// named separately because the two send an operator to different places — one
/// is a full disk, the other is a host that cannot read its own policy.
pub const DISK_CLEANUP_POLICY_UNKNOWN: &str = "disk_cleanup_policy_unknown";

/// `stado queue pause` is in effect; the agent publishes this per tick.
pub const QUEUE_PAUSED: &str = "queue_paused";

/// The registry pinned this host, so it claims only work addressed to it.
pub const PINNED_ONLY: &str = "pinned_only";

/// No capacity publication for this host exists at all. Not the agent's word,
/// because a silent agent has no words: the scheduler cannot see this host, so
/// it cannot be given anything.
pub const NO_CAPACITY_PUBLICATION: &str = "no_capacity_publication";

/// A publication older than [`capacity::CAPACITY_STALE_SECONDS`], which every
/// live-capacity reader in the fleet filters out. The row is reported anyway,
/// with its age, because "the agent said this an hour ago" and "nobody ever
/// said anything" are different findings.
pub const CAPACITY_PUBLICATION_STALE: &str = "capacity_publication_stale";

/// Everything one host answered about whether it can claim.
#[derive(Debug, Clone, PartialEq)]
pub struct HostGates {
    /// The registry target name, not the operator's spelling of it.
    pub host: String,
    /// `blockers.is_empty()`. Busy slots are NOT a blocker: a host running
    /// work claims nothing more and is perfectly healthy, and calling that
    /// blocked would make this command cry wolf on every loaded box.
    pub claiming: bool,
    pub blockers: Vec<String>,
    pub disk_pressure_unresolved: bool,
    /// `df -Pk /` available blocks as GiB, one decimal.
    pub free_gb: Option<f64>,
    /// The threshold admission is actually gated on: the janitor's own
    /// validated watermark first, the registry declaration second — the same
    /// order the agent resolves it in.
    pub low_watermark_gb: Option<i64>,
    pub target_free_gb: Option<i64>,
    pub policy_mode: Option<String>,
    pub published_at: Option<String>,
    pub age_seconds: Option<i64>,
    /// Free slots summed over accelerator types, or `None` when this host has
    /// published nothing.
    pub free_slots: Option<i64>,
    pub slots_declared: i64,
}

/// One capacity publication, with the instant it was made.
struct Publication {
    payload: Value,
    stamp: Option<DateTime<Utc>>,
}

/// Read every gate that decides whether `host` claims.
///
/// One ssh read and one object read, in that order. An unreachable host is an
/// error carrying the remote's own last line rather than a report full of
/// nulls: "this box is not answering" is a different answer from "this box is
/// answering and refuses to claim", and only the second one is what this
/// command was written to find.
pub async fn read_host_gates(host: &str, runner: &Runner) -> Result<HostGates, DeployError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let target = host_channel::resolve_target(&registry, host)?.clone();

    let interval = target
        .disk_cleanup
        .as_ref()
        .map(|policy| policy.check_interval_seconds);
    let output = host_channel::run_script(&target, &host_disk::remote_script(), runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the host did not report its disk state",
        )));
    }
    let reading = host_disk::parse_output(&output.stdout, interval);

    let store = JobStorage::new()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let publication = publication(&registry, &target, &store).await?;

    Ok(assemble(
        &target,
        &reading,
        publication.as_ref(),
        Utc::now(),
    ))
}

/// Join the three sources into the verdict.
///
/// Split out from the reads so the truth table is exercisable without a host,
/// a registry or a store.
fn assemble(
    target: &ComputeTarget,
    reading: &host_disk::DiskReading,
    publication: Option<&Publication>,
    now: DateTime<Utc>,
) -> HostGates {
    let policy = target.disk_cleanup.as_ref();
    let free_kb = reading
        .usage
        .as_ref()
        .and_then(|usage| usage.available_kb.parse::<f64>().ok());
    let free_gb = free_kb.map(host_disk::gib_from_blocks);
    // The janitor's validated watermark first: that is the number the agent
    // gated on, and it survives a registry the host could not read.
    let low_watermark_gb = reading
        .state
        .low_bytes
        .map(|bytes| bytes / disk_cleanup::GIB)
        .or_else(|| policy.map(|policy| policy.low_free_gb));

    let payload = publication.map(|row| &row.payload);
    let published_at = payload
        .and_then(|payload| payload.get("published_at"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let age_seconds = publication
        .and_then(|row| row.stamp)
        .map(|stamp| (now - stamp).num_seconds());
    let stale = age_seconds.is_some_and(|age| age > capacity::CAPACITY_STALE_SECONDS as i64);

    // The published verdict while the row is live — that IS the decision the
    // agent is making right now. Once the row is stale or absent the agent is
    // no longer talking, so the same function it uses is applied to the
    // numbers this command just measured itself.
    let published_pressure = diag_flag(payload, DISK_PRESSURE_UNRESOLVED);
    let disk_pressure_unresolved = match published_pressure {
        Some(published) if !stale => published,
        _ => disk_cleanup::disk_pressure_unresolved(
            low_watermark_gb.map(|gb| gb * disk_cleanup::GIB),
            free_kb.map(|blocks| (blocks * 1024.0) as i64),
        ),
    };

    let mut blockers: Vec<String> = Vec::new();
    if publication.is_none() {
        blockers.push(NO_CAPACITY_PUBLICATION.to_string());
    } else if stale {
        blockers.push(CAPACITY_PUBLICATION_STALE.to_string());
    }
    if disk_pressure_unresolved {
        blockers.push(DISK_PRESSURE_UNRESOLVED.to_string());
    }
    if diag_flag(payload, "disk_cleanup_policy_known") == Some(false) || low_watermark_gb.is_none()
    {
        blockers.push(DISK_CLEANUP_POLICY_UNKNOWN.to_string());
    }
    if diag_flag(payload, QUEUE_PAUSED) == Some(true) {
        blockers.push(QUEUE_PAUSED.to_string());
    }
    if diag_flag(payload, PINNED_ONLY) == Some(true) || target.pinned_only {
        blockers.push(PINNED_ONLY.to_string());
    }

    HostGates {
        host: target.name.clone(),
        claiming: blockers.is_empty(),
        blockers,
        disk_pressure_unresolved,
        free_gb,
        low_watermark_gb,
        target_free_gb: policy.map(|policy| policy.target_free_gb),
        policy_mode: policy.map(|policy| policy.mode.clone()),
        published_at,
        age_seconds,
        free_slots: payload.map(free_slots),
        slots_declared: target.slots,
    }
}

/// One `diag` boolean the agent published, or `None` when this host published
/// nothing or that tick carried no such key.
fn diag_flag(payload: Option<&Value>, key: &str) -> Option<bool> {
    payload
        .and_then(|payload| payload.get("diag"))
        .and_then(|diag| diag.get(key))
        .and_then(Value::as_bool)
}

/// Free slots across every accelerator type the host broadcast.
fn free_slots(payload: &Value) -> i64 {
    payload
        .get("free_slots")
        .and_then(Value::as_object)
        .map_or(0, |slots| {
            slots.values().filter_map(Value::as_i64).sum::<i64>()
        })
}

/// This host's capacity publication, stale ones included.
///
/// [`capacity::read_consumer_capacity`] cannot be used here: it drops
/// everything past the staleness horizon and garbage-collects what is past the
/// GC horizon, which is correct for a scheduler and exactly wrong for the
/// question being asked. A host whose agent went quiet an hour ago is the case
/// this command has to be able to report, not the case it deletes.
///
/// The consumer id is `<kind>-<hostname>`
/// ([`crate::providers::local::agent`]), and the hostname a host publishes is
/// its own, which need not be its registry name. So the identity is put back
/// through [`Registry::lookup_self`] — the fleet's one hostname-to-target
/// resolution — and only the row that resolves to THIS target is downloaded.
async fn publication(
    registry: &Registry,
    target: &ComputeTarget,
    store: &JobStorage,
) -> Result<Option<Publication>, DeployError> {
    let prefix = format!("{}-", target.kind);
    let blobs = store
        .list_blobs_with_meta(capacity::CAPACITY_PREFIX)
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    for blob in blobs {
        let Some(identity) = blob
            .name
            .strip_prefix(capacity::CAPACITY_PREFIX)
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|stem| stem.strip_prefix(prefix.as_str()))
        else {
            continue;
        };
        let mine = registry
            .lookup_self(identity)
            .map_err(|exc| DeployError(exc.to_string()))?
            .is_some_and(|found| found.name == target.name);
        if !mine {
            continue;
        }
        let Some(raw) = store
            .download_text(&blob.name)
            .await
            .map_err(|exc| DeployError(exc.to_string()))?
        else {
            continue;
        };
        let payload: Value = serde_json::from_str(&raw)
            .map_err(|exc| DeployError(format!("{} is not a capacity row: {exc}", blob.name)))?;
        // The row says when it was made; the object's own timestamp is the
        // fallback for a body that predates the field or carries an
        // unparseable one, so a row can never be reported as ageless.
        let stamp = payload
            .get("published_at")
            .and_then(Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|stamp| stamp.with_timezone(&Utc))
            .or(blob.updated);
        return Ok(Some(Publication { payload, stamp }));
    }
    Ok(None)
}

/// The `--json` report, in the exact shape the operator console consumes.
pub fn to_report(gates: &HostGates) -> Map<String, Value> {
    let mut report = Map::new();
    report.insert("host".to_string(), Value::String(gates.host.clone()));
    report.insert("claiming".to_string(), Value::Bool(gates.claiming));
    report.insert(
        "blockers".to_string(),
        Value::Array(
            gates
                .blockers
                .iter()
                .map(|blocker| Value::String(blocker.clone()))
                .collect(),
        ),
    );
    report.insert(
        "disk".to_string(),
        json!({
            "free_gb": gates.free_gb,
            "low_watermark_gb": gates.low_watermark_gb,
            "target_free_gb": gates.target_free_gb,
            "policy_mode": gates.policy_mode,
        }),
    );
    report.insert(
        "capacity".to_string(),
        json!({
            "published_at": gates.published_at,
            "age_seconds": gates.age_seconds,
            "free_slots": gates.free_slots,
            "slots_declared": gates.slots_declared,
        }),
    );
    report
}

/// The three fields a release verdict embeds when it has to say why a host is
/// not building anything.
///
/// Exported so `stado release doctor` reports the claiming gates from this
/// reader instead of growing a second one: two readers of
/// `capacity/<consumer>.json` would eventually give two answers to one
/// question, and the operator would believe whichever they ran first.
pub fn gates_section(gates: &HostGates) -> Value {
    json!({
        "disk_pressure_unresolved": gates.disk_pressure_unresolved,
        "free_gb": gates.free_gb,
        "low_watermark_gb": gates.low_watermark_gb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry target built the way every other deploy test builds one:
    /// through the deserializer, so the defaults are the registry's own.
    fn target(low_free_gb: i64) -> ComputeTarget {
        serde_json::from_value(json!({
            "name": "charless-mac-mini",
            "kind": "local",
            "ssh": "charles@charless-mac-mini.local",
            "release_platform": "darwin-arm64",
            "hostnames": ["charless-mac-mini.local"],
            "slots": 2,
            "disk_cleanup": {
                "mode": "enforce",
                "check_interval_seconds": 300,
                "low_free_gb": low_free_gb,
                "target_free_gb": low_free_gb + 25,
                "max_bytes_per_pass": 1_073_741_824_i64,
                "max_items_per_pass": 100,
                "max_scan_items": 10_000,
                "cleaners": {},
            },
        }))
        .expect("registry target")
    }

    /// `available_kb` is what `df -Pk` printed; the state's `low_bytes` is what
    /// the janitor validated.
    fn reading(available_kb: &str, low_bytes: Option<i64>) -> host_disk::DiskReading {
        host_disk::DiskReading {
            usage: Some(host_disk::DiskUsage {
                available_kb: available_kb.to_string(),
                ..host_disk::DiskUsage::default()
            }),
            state: host_disk::CleanupState {
                present: low_bytes.is_some(),
                low_bytes,
                ..host_disk::CleanupState::default()
            },
        }
    }

    fn publication(diag: Value, age_seconds: i64) -> Publication {
        let stamp = Utc::now() - chrono::TimeDelta::seconds(age_seconds);
        Publication {
            payload: json!({
                "consumer_id": "local-charless-mac-mini",
                "kind": "local",
                "free_slots": {"none": 0},
                "published_at": stamp.to_rfc3339(),
                "diag": diag,
            }),
            stamp: Some(stamp),
        }
    }

    /// The incident, as the command now reports it: 2 GiB free against a
    /// 55 GiB policy, the agent publishing its own refusal, and the CLI saying
    /// so in the agent's own word.
    #[test]
    fn the_mac_mini_incident_is_one_payload() {
        let row = publication(
            json!({"disk_pressure_unresolved": true, "disk_cleanup_policy_known": true}),
            30,
        );
        let gates = assemble(
            &target(55),
            &reading("2097152", Some(55 * disk_cleanup::GIB)),
            Some(&row),
            Utc::now(),
        );
        assert!(!gates.claiming);
        assert_eq!(gates.blockers, vec![DISK_PRESSURE_UNRESOLVED]);
        assert_eq!(gates.free_gb, Some(2.0));
        assert_eq!(gates.low_watermark_gb, Some(55));
        assert_eq!(gates.free_slots, Some(0));
        assert_eq!(gates.slots_declared, 2);
        assert_eq!(
            gates_section(&gates),
            json!({
                "disk_pressure_unresolved": true,
                "free_gb": 2.0,
                "low_watermark_gb": 55,
            })
        );
    }

    /// A host with room, a live row and nothing pinned is claiming, and the
    /// report says exactly that.
    #[test]
    fn a_healthy_host_reports_no_blockers() {
        let row = publication(
            json!({"disk_pressure_unresolved": false, "disk_cleanup_policy_known": true}),
            12,
        );
        let gates = assemble(
            &target(55),
            &reading("209715200", Some(55 * disk_cleanup::GIB)),
            Some(&row),
            Utc::now(),
        );
        assert!(gates.claiming);
        assert!(gates.blockers.is_empty());
        assert_eq!(gates.free_gb, Some(200.0));
        assert_eq!(gates.age_seconds, Some(12));
    }

    /// A silent agent is a blocker of its own, and the pressure verdict falls
    /// back to the numbers this command measured rather than to an absent one.
    #[test]
    fn a_host_that_published_nothing_is_not_claiming() {
        let gates = assemble(
            &target(55),
            &reading("2097152", Some(55 * disk_cleanup::GIB)),
            None,
            Utc::now(),
        );
        assert_eq!(
            gates.blockers,
            vec![NO_CAPACITY_PUBLICATION, DISK_PRESSURE_UNRESOLVED]
        );
        assert!(gates.disk_pressure_unresolved);
        assert_eq!(gates.free_slots, None);
        assert_eq!(gates.published_at, None);
    }

    /// A stale row is reported WITH its age, and its verdict is no longer
    /// trusted: the disk has room now, so the locally measured answer wins.
    #[test]
    fn a_stale_row_is_reported_and_recomputed() {
        let row = publication(json!({"disk_pressure_unresolved": true}), 3_600);
        let gates = assemble(
            &target(55),
            &reading("209715200", Some(55 * disk_cleanup::GIB)),
            Some(&row),
            Utc::now(),
        );
        assert_eq!(gates.blockers, vec![CAPACITY_PUBLICATION_STALE]);
        assert!(!gates.disk_pressure_unresolved);
        assert_eq!(gates.age_seconds, Some(3_600));
    }

    /// No watermark anywhere: the agent cannot prove there is room, so both
    /// words are reported, and neither is invented.
    #[test]
    fn an_unknown_policy_names_both_gates() {
        let mut target = target(55);
        target.disk_cleanup = None;
        let row = publication(
            json!({"disk_pressure_unresolved": true, "disk_cleanup_policy_known": false}),
            10,
        );
        let gates = assemble(&target, &reading("209715200", None), Some(&row), Utc::now());
        assert_eq!(
            gates.blockers,
            vec![DISK_PRESSURE_UNRESOLVED, DISK_CLEANUP_POLICY_UNKNOWN]
        );
        assert_eq!(gates.low_watermark_gb, None);
        assert_eq!(gates.policy_mode, None);
    }

    /// The report carries the contract's keys and only those: a desktop
    /// consumer reads this shape. Compared as a sorted set, because the
    /// document is printed through `host recover`'s sorted-keys printer and
    /// insertion order is not part of the contract.
    #[test]
    fn the_report_shape_is_the_contract() {
        let gates = assemble(&target(55), &reading("2097152", None), None, Utc::now());
        let report = to_report(&gates);
        let sorted = |value: &Value| -> Vec<String> {
            let mut keys: Vec<String> = value
                .as_object()
                .expect("a report section")
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        };
        assert_eq!(
            sorted(&Value::Object(report.clone())),
            ["blockers", "capacity", "claiming", "disk", "host"]
        );
        assert_eq!(
            sorted(&report["disk"]),
            [
                "free_gb",
                "low_watermark_gb",
                "policy_mode",
                "target_free_gb"
            ]
        );
        assert_eq!(
            sorted(&report["capacity"]),
            [
                "age_seconds",
                "free_slots",
                "published_at",
                "slots_declared"
            ]
        );
    }
}
