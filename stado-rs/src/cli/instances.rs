//! `stado instances` — cross-provider agent-VM inventory and orphan reaper.
//!
//! NO Python original: the Python CLI never exposed the running cloud fleet
//! at all. Both providers that can enumerate their VMs
//! (`providers/gcp/mod.rs::GcpProvider::list_running_instance_refs_with_age`
//! and `providers/azure/mod.rs::AzureProvider::list_running_instance_refs_with_age`)
//! were reachable only from `monitor/monitor.rs::reap_dead_agents`, which
//! runs inside the coordinator tick and prints nothing an operator can read.
//! A VM whose agent died therefore billed silently until someone opened the
//! cloud console — the July host incident and the GCP-billing outage were
//! both found that way.
//!
//! Uniform provider access: enumeration rides the existing optional trait
//! method `providers/mod.rs::Provider::list_running_instance_refs_with_age`
//! rather than provider-specific match arms here.
//!   * gcp — overrides it (aggregated list, non-TERMINATED `<prefix>-agent-*`).
//!   * azure — overrides it, forwarding to the inherent method.
//!   * box — inherits the base default; a box is rented per job through
//!     `queue/leases.rs`, there is no standing VM fleet to sweep.
//!   * aws — inherits the base default (empty). `providers/aws.rs::Ec2Api`
//!     exposes only `running_instance_types`, no per-instance enumeration,
//!     so an AWS row cannot be produced without widening that trait. AWS
//!     therefore reports an empty fleet here, and `instances list` names
//!     every provider that reported nothing rather than letting an
//!     unenumerable cloud render as "no orphans".
//!   * vast — not a `Provider` at all (wisent-compute is the marketplace
//!     HOST there, see `providers/vast.rs`), so it has no fleet to list.
//!
//! Ownership cross-check: a VM is an ORPHAN when nothing in the store still
//! claims it — no `running/` job document carries it as `instance_ref`, and
//! no un-released `provider-leases/` blob names it as its
//! `provider_resource_id`. That is the column the operator is looking for;
//! everything else on the row exists to justify it.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use clap::{Args, Subcommand};
use futures::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};

use crate::config;
use crate::providers::{get_provider, Provider};
use crate::queue::capacity::read_consumer_capacity;
use crate::queue::leases::{LeaseError, LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::migrations::BULK_WORKERS;
use crate::queue::JobStorage;

use super::{table, CmdError};

/// `JobStorage::list_jobs` / `list_paths` take `oldest_first = 0` for "no
/// bound" (Python `limit=None`); naming the sentinel keeps the call sites
/// honest about what zero means there.
const UNBOUNDED: usize = usize::MIN;

/// The blob prefix `queue/leases.rs::ProviderLeaseStore::path` writes under.
/// Already carried by `queue/copy.rs::CANONICAL_PREFIXES`; these commands
/// only read it.
const LEASE_PREFIX: &str = "provider-leases/";

/// Suffix of a lease blob name (`provider-leases/{job_id}.json`).
const LEASE_SUFFIX: &str = ".json";

/// `WC_PROVIDERS` entry for device-local agents. Skipped exactly as
/// `coordinator.rs::resolve_providers` skips it: a physical lab box has no
/// VM lifecycle to list or reap.
const LOCAL_PROVIDER: &str = "local";

/// Printed where a value could not be resolved from any source.
const UNKNOWN: &str = "-";

#[derive(Subcommand)]
pub enum InstancesCommands {
    /// List every live agent VM across the configured providers, flagging
    /// the ones no queue job or lease still references.
    List(InstancesListArgs),

    /// Delete orphaned agent VMs. Dry run unless `--yes` is given.
    Reap(InstancesReapArgs),
}

#[derive(Args, Debug)]
pub struct InstancesListArgs {
    /// Single provider to inspect; default is every entry in WC_PROVIDERS.
    #[arg(long)]
    provider: Option<String>,
    /// Emit machine-readable JSON instead of the table.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct InstancesReapArgs {
    /// Single provider to reap; default is every entry in WC_PROVIDERS.
    #[arg(long)]
    provider: Option<String>,
    /// Only consider VMs at least this old, as a duration string
    /// ("45s", "30m", "2h", "1d"). Omit to consider every orphan.
    #[arg(long = "older-than", value_parser = parse_older_than)]
    older_than: Option<Duration>,
    /// Report what would be deleted and delete nothing. This is already
    /// the default; passing it explicitly overrides `--yes`.
    #[arg(long)]
    dry_run: bool,
    /// Actually delete. Without this flag the command is a dry run.
    #[arg(long)]
    yes: bool,
    /// Emit machine-readable JSON instead of the tables.
    #[arg(long)]
    json: bool,
}

pub async fn dispatch(cmd: InstancesCommands) -> Result<(), CmdError> {
    match cmd {
        InstancesCommands::List(args) => list(&args).await,
        InstancesCommands::Reap(args) => reap(&args).await,
    }
}

// ---------------------------------------------------------------------------
// flag parsing
// ---------------------------------------------------------------------------

/// `--older-than` parse: `<count><unit>` with unit in s/m/h/d.
///
/// A bare number is rejected on purpose. "`--older-than 30`" reads as thirty
/// of whatever the reader assumes, and the flag gates deletions — the unit
/// is part of the operator's intent, not a default this command gets to
/// pick. `chrono` does the unit arithmetic, so the conversion factors live
/// in one audited place instead of here.
pub(crate) fn parse_older_than(raw: &str) -> Result<Duration, String> {
    let text = raw.trim();
    let units: &[(&str, fn(i64) -> Option<Duration>)] = &[
        ("s", Duration::try_seconds),
        ("m", Duration::try_minutes),
        ("h", Duration::try_hours),
        ("d", Duration::try_days),
    ];
    for &(suffix, build) in units {
        let Some(head) = text.strip_suffix(suffix) else {
            continue;
        };
        // u32 rejects a negative count at parse time; "older than minus an
        // hour" would select the whole fleet.
        let count: u32 = head.trim().parse().map_err(|_| {
            format!("{raw:?} is not a duration — expected a count and a unit, e.g. 30m")
        })?;
        return build(i64::from(count))
            .ok_or_else(|| format!("{raw:?} does not fit in a duration"));
    }
    Err(format!(
        "{raw:?} needs a unit suffix: s, m, h or d (e.g. 30m, 2h, 1d)"
    ))
}

/// Providers that own an agent-VM fleet: the single `--provider` name, or
/// every `WC_PROVIDERS` entry, minus [`LOCAL_PROVIDER`].
fn fleet_providers(selected: Option<&str>) -> Result<Vec<String>, CmdError> {
    let configured: Vec<String> = match selected {
        Some(name) => vec![name.trim().to_string()],
        None => config::wc_providers().to_vec(),
    };
    let fleet: Vec<String> = configured
        .into_iter()
        .filter(|name| name.as_str() != LOCAL_PROVIDER)
        .collect();
    if fleet.is_empty() {
        return Err(CmdError::click(
            "no provider with an agent-VM fleet selected (\"local\" agents run on physical \
             hosts and have no VM lifecycle)",
        ));
    }
    Ok(fleet)
}

// ---------------------------------------------------------------------------
// store-side ownership index
// ---------------------------------------------------------------------------

/// Everything in the store that can legitimately hold an agent VM, keyed by
/// the reference string the holder itself recorded.
struct Holders {
    /// reference-as-recorded -> human reasons ("job 1a2b3c4d", "lease ...").
    reasons: BTreeMap<String, Vec<String>>,
    /// reference-as-recorded -> the owning job's `gpu_type`.
    gpu_types: BTreeMap<String, String>,
}

impl Holders {
    async fn build(store: &JobStorage) -> Result<Self, CmdError> {
        let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut gpu_types: BTreeMap<String, String> = BTreeMap::new();

        // Running job documents. `instance_ref` is written by the dispatcher
        // as the provider ref and by local agents as "local@<hostname>";
        // both forms are matched in `Holders::keys`.
        for job in store.list_jobs("running", UNBOUNDED).await? {
            let Some(reference) = job.instance_ref.filter(|value| !value.is_empty()) else {
                continue;
            };
            reasons
                .entry(reference.clone())
                .or_default()
                .push(format!("job {}", job.job_id));
            if !job.gpu_type.is_empty() {
                gpu_types.entry(reference).or_insert(job.gpu_type);
            }
        }

        // Provider leases. `queue/leases.rs::ProviderLeaseStore` has no bulk
        // listing, so the job ids come from the blob names and each lease is
        // then loaded through the public `load` (which owns decoding and the
        // size bound).
        let lease_store = ProviderLeaseStore::new(store.clone());
        let job_ids: Vec<String> = store
            .list_paths(LEASE_PREFIX, UNBOUNDED)
            .await?
            .iter()
            .filter_map(|path| lease_job_id(path))
            .collect();
        // A lease that cannot be read is not a lease that can be ignored:
        // it may be the only record of who owns a VM, so the whole
        // inventory fails rather than authorizing a deletion on a partial
        // ownership picture.
        let loaded: Vec<Option<ProviderLease>> = futures::stream::iter(&job_ids)
            .map(|job_id| lease_store.load(job_id))
            .buffered(BULK_WORKERS)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<Option<ProviderLease>>, _>>()
            .map_err(|err: LeaseError| CmdError::click(err.to_string()))?;
        for lease in loaded.into_iter().flatten() {
            if lease.provider_resource_id.is_empty() || !lease_holds_resource(&lease) {
                continue;
            }
            reasons
                .entry(lease.provider_resource_id.clone())
                .or_default()
                .push(format!("lease {} ({})", lease.job_id, lease.state));
        }

        Ok(Holders { reasons, gpu_types })
    }

    /// Every string a holder may have recorded for one VM: the provider's
    /// own `name@zone`, the `local@<hostname>` form an agent stamps onto the
    /// job it claims (both are checked by
    /// `monitor/monitor.rs::reap_dead_agents`), and the bare VM name a lease
    /// records as its `provider_resource_id`.
    fn keys(reference: &str, vm_name: &str) -> Vec<String> {
        vec![
            reference.to_string(),
            format!("local@{vm_name}"),
            vm_name.to_string(),
        ]
    }

    fn holders_for(&self, reference: &str, vm_name: &str) -> Vec<String> {
        let mut found: Vec<String> = Self::keys(reference, vm_name)
            .iter()
            .filter_map(|key| self.reasons.get(key))
            .flatten()
            .cloned()
            .collect();
        found.sort();
        found.dedup();
        found
    }

    fn gpu_type_for(&self, reference: &str, vm_name: &str) -> Option<String> {
        Self::keys(reference, vm_name)
            .iter()
            .find_map(|key| self.gpu_types.get(key))
            .cloned()
    }
}

/// `provider-leases/{job_id}.json` -> `job_id`.
fn lease_job_id(path: &str) -> Option<String> {
    let job_id = path
        .strip_prefix(LEASE_PREFIX)?
        .strip_suffix(LEASE_SUFFIX)?;
    (!job_id.is_empty()).then(|| job_id.to_string())
}

/// Whether a lease still holds its provider resource: not released, and its
/// resource TTL has not lapsed.
///
/// An absent or unparseable `resource_expires_at` counts as HELD. The reaper
/// must never delete a VM on the strength of a timestamp it could not read.
fn lease_holds_resource(lease: &ProviderLease) -> bool {
    if lease.state == LeaseState::Released.as_str() {
        return false;
    }
    // Python `datetime.fromisoformat(value.replace("Z", "+00:00"))`, the
    // same normalization `queue/leases.rs::parse_timestamp` applies.
    match DateTime::parse_from_rfc3339(&lease.resource_expires_at.replace('Z', "+00:00")) {
        Ok(expires) => Utc::now() < expires.with_timezone(&Utc),
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// fleet inventory
// ---------------------------------------------------------------------------

/// One live agent VM plus everything known about who owns it.
struct InstanceRow {
    reference: String,
    provider: String,
    accel: String,
    age_seconds: f64,
    /// Reasons this VM is still claimed. Empty = orphan.
    held_by: Vec<String>,
}

impl InstanceRow {
    fn is_orphan(&self) -> bool {
        self.held_by.is_empty()
    }
}

/// The whole cross-provider picture: rows, the provider clients that
/// produced them (reused for deletion), and per-provider enumeration
/// failures. A provider that could not be reached yields an error entry, not
/// an empty fleet — "no VMs" and "no answer" must never look the same to an
/// operator hunting a runaway bill.
struct Fleet {
    rows: Vec<InstanceRow>,
    clients: BTreeMap<String, Arc<dyn Provider>>,
    errors: BTreeMap<String, String>,
}

impl Fleet {
    fn rows_for<'a>(&'a self, provider: &'a str) -> impl Iterator<Item = &'a InstanceRow> {
        self.rows.iter().filter(move |row| row.provider == provider)
    }
}

async fn inventory(store: &JobStorage, providers: &[String]) -> Result<Fleet, CmdError> {
    let live = read_consumer_capacity(store).await?;
    inventory_with_live(store, providers, &live).await
}

async fn inventory_with_live(
    store: &JobStorage,
    providers: &[String],
    live: &BTreeMap<String, Value>,
) -> Result<Fleet, CmdError> {
    let holders = Holders::build(store).await?;
    let agent_prefix = format!("{}-agent-", config::INSTANCE_PREFIX);

    let mut fleet = Fleet {
        rows: Vec::new(),
        clients: BTreeMap::new(),
        errors: BTreeMap::new(),
    };
    for name in providers {
        let client = match get_provider(name) {
            Ok(client) => client,
            Err(err) => {
                fleet.errors.insert(name.clone(), err.to_string());
                continue;
            }
        };
        let refs = match client.list_running_instance_refs_with_age().await {
            Ok(refs) => refs,
            Err(err) => {
                fleet.errors.insert(name.clone(), err.to_string());
                continue;
            }
        };
        fleet.clients.insert(name.clone(), client);
        for (reference, age_seconds) in refs {
            let vm_name = reference.split('@').next().unwrap_or_default().to_string();
            let held_by = holders.holders_for(&reference, &vm_name);
            let accel = holders
                .gpu_type_for(&reference, &vm_name)
                .or_else(|| broadcast_accel(live, name, &vm_name))
                .or_else(|| name_tag_accel(&agent_prefix, &vm_name))
                .unwrap_or_else(|| UNKNOWN.to_string());
            fleet.rows.push(InstanceRow {
                reference,
                provider: name.clone(),
                accel,
                age_seconds,
                held_by,
            });
        }
    }
    // Orphans first, then oldest first: the top of the table is the money.
    fleet.rows.sort_by(|left, right| {
        right
            .is_orphan()
            .cmp(&left.is_orphan())
            .then_with(|| right.age_seconds.total_cmp(&left.age_seconds))
            .then_with(|| left.reference.cmp(&right.reference))
    });
    Ok(fleet)
}

/// Read-only fleet projection for resource rationalization. Unlike the
/// operator list/reaper path, this deliberately skips live-capacity loading,
/// whose stale-record GC would make an audit mutate the store.
pub(crate) async fn audit_inventory(
    store: &JobStorage,
    providers: &[String],
) -> Result<AuditFleet, CmdError> {
    let fleet = inventory_with_live(store, providers, &BTreeMap::new()).await?;
    Ok(AuditFleet {
        rows: fleet
            .rows
            .into_iter()
            .map(|row| AuditInstanceRow {
                reference: row.reference,
                provider: row.provider,
                accel: row.accel,
                age_seconds: row.age_seconds,
                held_by: row.held_by,
            })
            .collect(),
        errors: fleet.errors,
    })
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuditInstanceRow {
    pub reference: String,
    pub provider: String,
    pub accel: String,
    pub age_seconds: f64,
    pub held_by: Vec<String>,
}

impl AuditInstanceRow {
    pub fn is_orphan(&self) -> bool {
        self.held_by.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuditFleet {
    pub rows: Vec<AuditInstanceRow>,
    pub errors: BTreeMap<String, String>,
}

/// Accelerator types from this VM's live capacity broadcast
/// (`queue/capacity.rs`, consumer id `<kind>-<vm name>`). Authoritative for
/// an agent that is still publishing; silent for the dead ones, which is
/// exactly when the name tag below has to answer.
fn broadcast_accel(
    live: &BTreeMap<String, Value>,
    provider: &str,
    vm_name: &str,
) -> Option<String> {
    let slots = live
        .get(&format!("{provider}-{vm_name}"))?
        .get("free_slots")?
        .as_object()?;
    let joined = slots.keys().cloned().collect::<Vec<String>>().join(",");
    (!joined.is_empty()).then_some(joined)
}

/// Last-resort accelerator label: the short tag the dispatcher bakes into
/// the VM name (`scheduler/dispatch/agent.rs` builds
/// `<prefix>-agent-<accel tail>-<tick>-<index>`, so "t4" / "80gb"). Not a
/// full accelerator type — it is what is knowable about a VM whose agent
/// never came up.
fn name_tag_accel(agent_prefix: &str, vm_name: &str) -> Option<String> {
    let tag = vm_name.strip_prefix(agent_prefix)?.split('-').next()?;
    (!tag.is_empty()).then(|| tag.to_string())
}

// ---------------------------------------------------------------------------
// instances list
// ---------------------------------------------------------------------------

async fn list(args: &InstancesListArgs) -> Result<(), CmdError> {
    let providers = fleet_providers(args.provider.as_deref())?;
    let store = JobStorage::new().await?;
    let fleet = inventory(&store, &providers).await?;

    if args.json {
        let instances: Vec<Value> = fleet
            .rows
            .iter()
            .map(|row| {
                json!({
                    "reference": row.reference,
                    "provider": row.provider,
                    "accel": row.accel,
                    "age_seconds": row.age_seconds,
                    "orphan": row.is_orphan(),
                    "held_by": row.held_by,
                })
            })
            .collect();
        echo_json(&json!({
            "providers": providers,
            "instances": instances,
            "errors": fleet.errors,
        }));
    } else {
        let rows: Vec<Vec<String>> = fleet
            .rows
            .iter()
            .map(|row| {
                vec![
                    row.reference.clone(),
                    row.provider.clone(),
                    row.accel.clone(),
                    format_age(row.age_seconds),
                    yes_no(row.is_orphan()).to_string(),
                    if row.held_by.is_empty() {
                        UNKNOWN.to_string()
                    } else {
                        row.held_by.join(", ")
                    },
                ]
            })
            .collect();
        table::print(
            &["REFERENCE", "PROVIDER", "ACCEL", "AGE", "ORPHAN", "HELD BY"],
            &rows,
        );
        let orphans = fleet.rows.iter().filter(|row| row.is_orphan()).count();
        println!("\n{} live VM(s), {orphans} orphan(s).", fleet.rows.len());
        for provider in &providers {
            if !fleet.errors.contains_key(provider) && fleet.rows_for(provider).next().is_none() {
                println!("{provider}: reported no agent VMs.");
            }
        }
        print_errors(&fleet.errors);
    }
    enumeration_result(&fleet.errors)
}

// ---------------------------------------------------------------------------
// instances reap
// ---------------------------------------------------------------------------

/// Per-provider outcome. Counts are list lengths rather than counters so the
/// report can name every reference it acted on (and skipped) instead of
/// making the operator guess which VM the number refers to.
#[derive(Default)]
struct Tally {
    would_delete: Vec<String>,
    deleted: Vec<String>,
    failed: Vec<(String, String)>,
    skipped: Vec<(String, String)>,
}

async fn reap(args: &InstancesReapArgs) -> Result<(), CmdError> {
    let providers = fleet_providers(args.provider.as_deref())?;
    let store = JobStorage::new().await?;
    let fleet = inventory(&store, &providers).await?;
    // `--dry-run` is the default and wins when both flags are given: the
    // explicit brake beats the explicit accelerator.
    let apply = args.yes && !args.dry_run;
    reap_fleet(&store, &providers, fleet, args.older_than, apply, args.json).await
}

/// Action backend for `kill-irrational-resources`.
///
/// Its preview is genuinely read-only: unlike the legacy operator reaper it
/// does not load or garbage-collect capacity publications. Ownership is
/// rebuilt immediately before every applied deletion.
pub(crate) async fn reap_irrational(
    providers: &[String],
    older_than: Duration,
    apply: bool,
    json: bool,
) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let fleet = inventory_with_live(&store, providers, &BTreeMap::new()).await?;
    reap_fleet(&store, providers, fleet, Some(older_than), apply, json).await
}

async fn reap_fleet(
    store: &JobStorage,
    providers: &[String],
    fleet: Fleet,
    older_than: Option<Duration>,
    apply: bool,
    json: bool,
) -> Result<(), CmdError> {
    let mut tallies: BTreeMap<String, Tally> = providers
        .iter()
        .map(|name| (name.clone(), Tally::default()))
        .collect();
    for row in &fleet.rows {
        let tally = tallies.entry(row.provider.clone()).or_default();
        if !row.held_by.is_empty() {
            tally.skipped.push((
                row.reference.clone(),
                format!("referenced by {}", row.held_by.join(", ")),
            ));
            continue;
        }
        if let Some(minimum) = older_than {
            // A provider that could not read a creation timestamp reports
            // age 0.0; such a VM never clears an age gate, which is the
            // safe direction for a deletion.
            if row.age_seconds < minimum.num_seconds() as f64 {
                tally.skipped.push((
                    row.reference.clone(),
                    format!("age {} is under --older-than", format_age(row.age_seconds)),
                ));
                continue;
            }
        }
        if !apply {
            tally.would_delete.push(row.reference.clone());
            continue;
        }
        let holders = Holders::build(store).await?;
        let vm_name = row.reference.split('@').next().unwrap_or_default();
        let held_by = holders.holders_for(&row.reference, vm_name);
        if !held_by.is_empty() {
            tally.skipped.push((
                row.reference.clone(),
                format!(
                    "ownership changed after planning; now referenced by {}",
                    held_by.join(", ")
                ),
            ));
            continue;
        }
        let Some(client) = fleet.clients.get(&row.provider) else {
            tally.failed.push((
                row.reference.clone(),
                "provider client is unavailable".to_string(),
            ));
            continue;
        };
        match client.delete_instance(&row.reference).await {
            Ok(()) => tally.deleted.push(row.reference.clone()),
            Err(err) => tally.failed.push((row.reference.clone(), err.to_string())),
        }
    }

    if json {
        report_json(providers, &fleet, &tallies, apply);
    } else {
        report_tables(&fleet, &tallies, apply);
    }

    let failures: Vec<String> = tallies
        .iter()
        .flat_map(|(provider, tally)| {
            tally
                .failed
                .iter()
                .map(move |(reference, _)| format!("{provider} {reference}"))
        })
        .collect();
    if !failures.is_empty() {
        return Err(CmdError::click(format!(
            "deletion failed for: {}",
            failures.join(", ")
        )));
    }
    enumeration_result(&fleet.errors)
}

fn report_json(
    providers: &[String],
    fleet: &Fleet,
    tallies: &BTreeMap<String, Tally>,
    apply: bool,
) {
    let per_provider: BTreeMap<String, Value> = tallies
        .iter()
        .map(|(provider, tally)| {
            (
                provider.clone(),
                json!({
                    "live": fleet.rows_for(provider).count(),
                    "orphans": fleet.rows_for(provider).filter(|row| row.is_orphan()).count(),
                    "would_delete": tally.would_delete,
                    "deleted": tally.deleted,
                    "failed": tally
                        .failed
                        .iter()
                        .map(|(reference, error)| json!({"reference": reference, "error": error}))
                        .collect::<Vec<Value>>(),
                    "skipped": tally
                        .skipped
                        .iter()
                        .map(|(reference, reason)| json!({"reference": reference, "reason": reason}))
                        .collect::<Vec<Value>>(),
                }),
            )
        })
        .collect();
    echo_json(&json!({
        "applied": apply,
        "providers": providers,
        "tally": per_provider,
        "errors": fleet.errors,
    }));
}

fn report_tables(fleet: &Fleet, tallies: &BTreeMap<String, Tally>, apply: bool) {
    let summary: Vec<Vec<String>> = tallies
        .iter()
        .map(|(provider, tally)| {
            vec![
                provider.clone(),
                fleet.rows_for(provider).count().to_string(),
                fleet
                    .rows_for(provider)
                    .filter(|row| row.is_orphan())
                    .count()
                    .to_string(),
                tally.would_delete.len().to_string(),
                tally.deleted.len().to_string(),
                tally.failed.len().to_string(),
                tally.skipped.len().to_string(),
            ]
        })
        .collect();
    table::print(
        &[
            "PROVIDER",
            "LIVE",
            "ORPHANS",
            "WOULD DELETE",
            "DELETED",
            "FAILED",
            "SKIPPED",
        ],
        &summary,
    );

    let skipped: Vec<Vec<String>> = tallies
        .iter()
        .flat_map(|(provider, tally)| {
            tally.skipped.iter().map(move |(reference, reason)| {
                vec![provider.clone(), reference.clone(), reason.clone()]
            })
        })
        .collect();
    if !skipped.is_empty() {
        table::print(&["PROVIDER", "SKIPPED", "WHY"], &skipped);
    }

    let failed: Vec<Vec<String>> = tallies
        .iter()
        .flat_map(|(provider, tally)| {
            tally.failed.iter().map(move |(reference, error)| {
                vec![provider.clone(), reference.clone(), error.clone()]
            })
        })
        .collect();
    if !failed.is_empty() {
        table::print(&["PROVIDER", "FAILED", "ERROR"], &failed);
    }

    if !apply {
        let pending: usize = tallies.values().map(|tally| tally.would_delete.len()).sum();
        println!(
            "\nDry run: nothing was deleted. Re-run with --yes to delete the {pending} \
             instance(s) listed under WOULD DELETE."
        );
    }
    print_errors(&fleet.errors);
}

// ---------------------------------------------------------------------------
// shared output helpers
// ---------------------------------------------------------------------------

/// A provider we could not enumerate is a hole in the inventory, so the
/// command exits non-zero even when everything it *could* see was fine.
/// Reporting "no orphans" for a cloud we never reached is the failure this
/// command exists to prevent.
fn enumeration_result(errors: &BTreeMap<String, String>) -> Result<(), CmdError> {
    if errors.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = errors.keys().map(String::as_str).collect();
    Err(CmdError::click(format!(
        "could not enumerate provider(s): {}",
        names.join(", ")
    )))
}

fn print_errors(errors: &BTreeMap<String, String>) {
    for (provider, message) in errors {
        println!("{provider}: ENUMERATION FAILED — {message}");
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

/// `<h>h<m>m` from a provider-reported age in seconds. Hours, not days: the
/// quantity the operator is reasoning about is billed GPU-hours. `chrono`
/// carries the unit arithmetic.
fn format_age(age_seconds: f64) -> String {
    let Some(total) = Duration::try_seconds(age_seconds as i64) else {
        return UNKNOWN.to_string();
    };
    let hours = total.num_hours();
    let rest = total - Duration::try_hours(hours).unwrap_or_else(Duration::zero);
    format!("{hours}h{}m", rest.num_minutes())
}

/// Python `click.echo(json.dumps(payload, indent=2, sort_keys=True))`, as in
/// `cli/quota.rs::echo_json`.
fn echo_json(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("Value serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}
