//! One fleet-level answer to "can anything claim this queue".
//!
//! NO Python original. The incident it exists for: job `2c4a47aa` sat in
//! `queue/` for 121 hours and no command in this product said why. `stado
//! status` listed it under a row of counts that end in "1 queued"; `stado
//! overview` printed a worker count next to the words "active workers"; and
//! the one fact that explained the stall — that not a single host in the
//! registry currently publishes capacity, so nothing can ever claim it —
//! existed only per-host, one ssh round trip at a time, behind `stado host
//! gates HOST`. An operator who did not already suspect a specific host had
//! no way to reach it.
//!
//! A queue with no claimant looks exactly like an empty queue. This module is
//! the difference, and it is a report: no exit status, no gate, nothing
//! written, nothing deleted.
//!
//! Three sources, joined here and re-derived nowhere:
//!
//! - every host's newest capacity publication, read through
//!   [`capacity::read_publications`] — the GC-free reader, because the
//!   scheduler's [`capacity::read_consumer_capacity`] deletes rows past its
//!   GC horizon and a report that destroys its own evidence would answer
//!   "nobody ever said anything" where the truth is "that host went quiet an
//!   hour ago";
//! - the queued jobs, for the wait that sizes the stall and for the pins that
//!   decide whether a `pinned_only` host is idle by policy or starving;
//! - the registry's declared units joined against the host's newest health
//!   beacon, for the commonest cause of the silence: a declared queue agent
//!   that nothing on the host is running.
//!
//! Every word an operator reads here is [`host_gates`]'s word for the same
//! condition, because a blocker that is greppable in one command and spelled
//! differently in another is two vocabularies for one fact.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::host_gates;
use super::service;
use super::DeployError;
use crate::models::Job;
use crate::monitor::host_health;
use crate::queue::capacity::{self, Publication};
use crate::queue::JobStorage;
use crate::targets::{ComputeTarget, Registry};

/// The `com.wisent.compute.agent.` label prefix
/// [`crate::deploy::local_install::label`] mints for `kind=agent`.
const MINTED_AGENT_PREFIX: &str = "com.wisent.compute.agent.";

/// The spelling an operator gets when they deploy a queue agent through
/// `stado service deploy`, which mints the `service` kind instead: the mini's
/// agent is declared as `com.wisent.compute.service.stado-agent-mini`.
const DEPLOYED_AGENT_MARK: &str = "stado-agent";

/// One reason one host cannot claim, in [`host_gates`]'s vocabulary, plus the
/// detail that sizes it.
///
/// The word and the detail are separate so the word stays greppable: an
/// operator who reads `no_capacity_publication` here has to be able to find
/// it in `stado host gates --json` and in the agent that would have published
/// it, and a word with an age or a unit label glued onto it is findable in
/// neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    /// A `host_gates` constant, verbatim.
    pub word: String,
    /// What makes it true here, in the operator's words. Empty when the word
    /// says everything.
    pub detail: String,
}

impl Blocker {
    fn new(word: &str, detail: impl Into<String>) -> Self {
        Self {
            word: word.to_string(),
            detail: detail.into(),
        }
    }

    fn bare(word: &str) -> Self {
        Self::new(word, "")
    }

    /// `word` or `word (detail)`.
    fn rendered(&self) -> String {
        if self.detail.is_empty() {
            self.word.clone()
        } else {
            format!("{} ({})", self.word, self.detail)
        }
    }
}

/// One declared host's answer to "could you claim something from this queue".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostVerdict {
    /// The registry target name.
    pub host: String,
    /// `blockers.is_empty()`. A host with free slots it is not using and a
    /// host with every slot busy both claim: busy is a moving queue, and
    /// calling it blocked would make this report cry wolf on every loaded
    /// box (the same rule [`host_gates::HostGates::claiming`] follows).
    pub claiming: bool,
    pub blockers: Vec<Blocker>,
}

/// The queued job that has waited longest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldestWait {
    pub job_id: String,
    /// `None` for a job whose `created_at` cannot be parsed; the job is still
    /// reported, without an age, rather than dropped from the count.
    pub age_seconds: Option<i64>,
}

/// Whether anything in this fleet can claim anything in this queue, and when
/// not, why not, host by host.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetClaim {
    /// How many jobs sit in `queue/`.
    pub queued: usize,
    /// The longest-waiting queued job, or `None` for an empty queue.
    pub oldest: Option<OldestWait>,
    /// Every `kind=local` registry target, in registry order.
    pub hosts: Vec<HostVerdict>,
    /// Registry names of the local hosts whose newest capacity publication is
    /// inside the staleness horizon. The number `stado overview` prints where
    /// it used to print a count of declared workers.
    pub publishing: Vec<String>,
    /// Consumer ids publishing capacity that no declared local target
    /// claims: cloud dispatchers, marketplace workers, and hosts the
    /// registry has not adopted. A fresh one of these is why this report
    /// refuses to say nothing can claim — it cannot see what that publisher
    /// is able to take, and asserting a stall it cannot prove is worse than
    /// saying less.
    pub unattributed: Vec<String>,
    /// consumer_id -> publication for every row in the store, stale rows
    /// included. Retained so a caller that also needs the live capacity
    /// rows — `stado overview` needs them for its worker list — reads the
    /// prefix once, through this reader, and can never disagree with the
    /// verdict about which rows are fresh.
    publications: BTreeMap<String, Publication>,
    now: DateTime<Utc>,
}

impl FleetClaim {
    /// {consumer_id: payload} for every publication inside the staleness
    /// horizon — what [`capacity::read_consumer_capacity`] would return, with
    /// nothing deleted on the way.
    pub fn live_consumers(&self) -> BTreeMap<String, Value> {
        self.publications
            .iter()
            .filter(|(_, row)| !row.stale(self.now))
            .map(|(id, row)| (id.clone(), row.payload.clone()))
            .collect()
    }

    /// At least one host could claim at least one queued job.
    ///
    /// True for an empty fleet-wide unknown: a fresh publisher this report
    /// cannot attribute to a declared host counts as a claimant, because
    /// "nothing can claim" is a strong statement and this reader must only
    /// make it when it can see every publisher that exists.
    pub fn claimable(&self) -> bool {
        self.hosts.iter().any(|host| host.claiming)
            || self.unattributed.iter().any(|id| {
                self.publications
                    .get(id)
                    .is_some_and(|row| !row.stale(self.now))
            })
    }

    /// Work is queued and nothing in the fleet can take it. The one condition
    /// worth interrupting an operator over.
    pub fn stuck(&self) -> bool {
        self.queued > 0 && !self.claimable()
    }

    /// The verdict as an operator reads it: a headline that sizes the stall,
    /// then one line per host that cannot claim, in the host's own words.
    ///
    /// Empty unless [`Self::stuck`]. One renderer for both surfaces, so
    /// `stado overview` and `stado status` can never print two different
    /// explanations of one stuck queue.
    pub fn lines(&self) -> Vec<String> {
        if !self.stuck() {
            return Vec::new();
        }
        let mut lines = vec![format!(
            "nothing can claim the queue: {} queued, {}; {} of {} local hosts publish capacity newer than {}s",
            self.queued,
            self.oldest_words(),
            self.publishing.len(),
            self.hosts.len(),
            capacity::CAPACITY_STALE_SECONDS,
        )];
        if self.hosts.is_empty() {
            lines.push("  cannot claim: the registry declares no kind=local host".to_string());
        }
        for host in &self.hosts {
            if host.claiming {
                continue;
            }
            let words: Vec<String> = host
                .blockers
                .iter()
                .map(|blocker| blocker.rendered())
                .collect();
            lines.push(format!(
                "  cannot claim: {} {}",
                host.host,
                words.join(", ")
            ));
        }
        lines
    }

    /// `oldest 2c4a47aa waiting 121h 38m`, or the empty-queue phrasing.
    fn oldest_words(&self) -> String {
        match &self.oldest {
            None => "nothing waiting".to_string(),
            Some(job) => match job.age_seconds {
                None => format!("oldest {} waiting an unreadable time", job.job_id),
                Some(age) => format!("oldest {} waiting {}", job.job_id, wait_words(age)),
            },
        }
    }

    /// The `--json` section.
    pub fn to_report(&self) -> Value {
        json!({
            "claimable": self.claimable(),
            "stuck": self.stuck(),
            "queued": self.queued,
            "oldest_queued": self.oldest.as_ref().map(|job| json!({
                "job_id": job.job_id,
                "age_seconds": job.age_seconds,
                "waited": job.age_seconds.map(wait_words),
            })),
            "stale_horizon_seconds": capacity::CAPACITY_STALE_SECONDS,
            "publishing": self.publishing,
            "unattributed_publishers": self.unattributed,
            "hosts": self.hosts.iter().map(|host| json!({
                "host": host.host,
                "claiming": host.claiming,
                "blockers": host.blockers.iter().map(|blocker| json!({
                    "word": blocker.word,
                    "detail": blocker.detail,
                })).collect::<Vec<Value>>(),
            })).collect::<Vec<Value>>(),
        })
    }
}

/// A queue wait as `121h 38m`.
///
/// Hours are never rolled into days, which is why this is not
/// [`crate::monitor::billing::humanize`]: `5d 1h 38m` reads as a backlog
/// being worked through, and the number that tells an operator this queue has
/// not moved at all is the hour count.
pub fn wait_words(seconds: i64) -> String {
    let total = u64::try_from(seconds).unwrap_or_default();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    if hours == 0 {
        return format!("{minutes}m");
    }
    format!("{hours}h {minutes}m")
}

/// Read the verdict.
///
/// One listing plus one body per capacity row, one queue listing, and one
/// beacon read per silent host that declares a queue agent. No ssh, so this
/// stays answerable while every host in the fleet is wedged — which is
/// exactly when it is asked.
pub async fn read_fleet_claim(
    store: &JobStorage,
    registry: &Registry,
    now: DateTime<Utc>,
) -> Result<FleetClaim, DeployError> {
    let publications = capacity::read_publications(store)
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let queued = store
        .list_jobs("queue", 0)
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;

    let mut hosts: Vec<HostVerdict> = Vec::new();
    let mut publishing: Vec<String> = Vec::new();
    let mut attributed: Vec<String> = Vec::new();
    for target in registry.local_targets() {
        let mut mine: Option<&Publication> = None;
        for (consumer_id, row) in &publications {
            if host_gates::resolves_to(registry, target, consumer_id)? {
                attributed.push(consumer_id.clone());
                mine = Some(row);
                break;
            }
        }
        if mine.is_some_and(|row| !row.stale(now)) {
            publishing.push(target.name.clone());
        }
        let blockers = host_blockers(store, registry, target, mine, &queued, now).await?;
        hosts.push(HostVerdict {
            host: target.name.clone(),
            claiming: blockers.is_empty(),
            blockers,
        });
    }

    let unattributed = publications
        .keys()
        .filter(|id| !attributed.contains(id))
        .cloned()
        .collect();

    Ok(FleetClaim {
        queued: queued.len(),
        oldest: oldest_wait(&queued, now),
        hosts,
        publishing,
        unattributed,
        publications,
        now,
    })
}

/// Every reason one host cannot claim, silence first and policy last: an
/// operator reads the top line to learn whether the host is talking at all,
/// and everything below it is only meaningful once it is.
async fn host_blockers(
    store: &JobStorage,
    registry: &Registry,
    target: &ComputeTarget,
    publication: Option<&Publication>,
    queued: &[Job],
    now: DateTime<Utc>,
) -> Result<Vec<Blocker>, DeployError> {
    let mut blockers: Vec<Blocker> = Vec::new();
    match publication {
        None => blockers.push(Blocker::bare(host_gates::NO_CAPACITY_PUBLICATION)),
        Some(row) if row.stale(now) => blockers.push(Blocker::new(
            host_gates::CAPACITY_PUBLICATION_STALE,
            match row.age_seconds(now) {
                Some(age) => format!("last published {} ago", wait_words(age)),
                None => "published at an unreadable time".to_string(),
            },
        )),
        Some(_) => {}
    }

    // The declaration is asked about only while the host is silent. A host
    // publishing fresh capacity is running an agent whatever its units are
    // named, and reporting a declaration finding against it would be a note
    // dressed as a blocker.
    if !blockers.is_empty() {
        if let Some(agent) = declared_agent(target) {
            if let Some(detail) = agent_not_loaded(store, target, &agent).await? {
                blockers.push(Blocker::new(host_gates::AGENT_DECLARED_NOT_LOADED, detail));
            }
        }
    }

    let payload = publication.map(|row| &row.payload);
    let fresh = publication.is_some_and(|row| !row.stale(now));
    if fresh && diag_flag(payload, host_gates::QUEUE_PAUSED) == Some(true) {
        blockers.push(Blocker::bare(host_gates::QUEUE_PAUSED));
    }
    if fresh && diag_flag(payload, host_gates::DISK_PRESSURE_UNRESOLVED) == Some(true) {
        blockers.push(Blocker::bare(host_gates::DISK_PRESSURE_UNRESOLVED));
    }

    // Pinned-only is a blocker only when the queue holds nothing addressed to
    // this host. A pinned host with a matching queued job is a host that
    // would claim, so the reason it is not claiming is one of the words
    // above, and printing `pinned_only` beside them would send an operator to
    // change a policy that is not the problem.
    let pinned_only =
        target.pinned_only || diag_flag(payload, host_gates::PINNED_ONLY) == Some(true);
    if pinned_only && !pinned_here(registry, target, queued)? {
        blockers.push(Blocker::new(
            host_gates::PINNED_ONLY,
            "no queued job names this host",
        ));
    }
    Ok(blockers)
}

/// The queue agent this target declares, if it declares one.
///
/// Two spellings exist in this fleet and both are the agent: the label
/// [`crate::deploy::local_install::label`] mints for `kind=agent`
/// ([`MINTED_AGENT_PREFIX`]), and the `service`-kind label an operator gets
/// from `stado service deploy` ([`DEPLOYED_AGENT_MARK`], as in the mini's
/// `com.wisent.compute.service.stado-agent-mini`).
fn declared_agent(target: &ComputeTarget) -> Option<service::ManagedService> {
    service::declared_services(target).into_iter().find(|unit| {
        unit.unit_id().starts_with(MINTED_AGENT_PREFIX)
            || unit.unit_id().contains(DEPLOYED_AGENT_MARK)
            || unit.name.contains(DEPLOYED_AGENT_MARK)
    })
}

/// The detail for [`host_gates::AGENT_DECLARED_NOT_LOADED`], or `None` when
/// the host's newest beacon reports the declared unit running.
///
/// Beacon-only, by the same rule [`service::list_services`] joins a
/// declaration to a beacon: the moment you most need to know what is supposed
/// to be running on a host is the moment that host has stopped answering ssh.
/// A host with no beacon at all yields `None` — that is a second silence, not
/// a claim about the unit, and [`host_gates::NO_CAPACITY_PUBLICATION`] has
/// already said the host is quiet.
async fn agent_not_loaded(
    store: &JobStorage,
    target: &ComputeTarget,
    agent: &service::ManagedService,
) -> Result<Option<String>, DeployError> {
    let unit = agent.unit_id();
    for slug in host_health::beacon_slugs(target, &target.name) {
        let path = format!("{}/{slug}.json", host_health::HEALTH_PREFIX);
        let Some(raw) = store
            .download_text(&path)
            .await
            .map_err(|exc| DeployError(exc.to_string()))?
        else {
            continue;
        };
        let beacon: Value =
            serde_json::from_str(&raw).map_err(|exc| DeployError(format!("{path}: {exc}")))?;
        let Some(units) = beacon.get("units").and_then(Value::as_object) else {
            return Ok(None);
        };
        let Some(entry) = units.get(unit) else {
            return Ok(Some(format!(
                "{unit} is declared at {}; the latest beacon does not report it",
                agent.path
            )));
        };
        // The beacon writer emits {"state": ...} per unit; older beacons
        // wrote a bare string. Both shapes are in flight, so read both --
        // the same two shapes `service::beacon_state` reads.
        let state = match entry {
            Value::String(state) => state.as_str(),
            Value::Object(fields) => fields.get("state").and_then(Value::as_str).unwrap_or(""),
            _ => "",
        };
        if state == service::STATE_ACTIVE {
            return Ok(None);
        }
        return Ok(Some(format!(
            "{unit} is declared at {}; the latest beacon reports it {}",
            agent.path,
            if state.is_empty() {
                "with no state"
            } else {
                state
            }
        )));
    }
    Ok(None)
}

/// Whether any queued job is addressed to this host.
///
/// A pinned job names its consumer as `<kind>-<hostname>`, and the hostname is
/// the machine's own word for itself, not its registry name — resolved the way
/// [`host_gates`] resolves it, through the fleet's one hostname-to-target
/// lookup. Jobs pinned by exact registry name are honored too, because the
/// operator-facing `--pinned-host` accepts that spelling.
fn pinned_here(
    registry: &Registry,
    target: &ComputeTarget,
    queued: &[Job],
) -> Result<bool, DeployError> {
    for job in queued {
        if job.pinned_host.is_empty() {
            continue;
        }
        if job.pinned_host == target.name
            || host_gates::resolves_to(registry, target, &job.pinned_host)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The longest-waiting queued job. A job whose `created_at` will not parse
/// sorts last rather than out: it is still queued, and dropping it would
/// shrink the count the headline prints.
fn oldest_wait(queued: &[Job], now: DateTime<Utc>) -> Option<OldestWait> {
    queued
        .iter()
        .map(|job| OldestWait {
            job_id: job.job_id.clone(),
            age_seconds: DateTime::parse_from_rfc3339(&job.created_at)
                .ok()
                .map(|created| (now - created.with_timezone(&Utc)).num_seconds()),
        })
        .max_by_key(|wait| wait.age_seconds.unwrap_or(i64::MIN))
}

/// One `diag` boolean the agent published, read exactly as
/// [`host_gates`] reads it.
fn diag_flag(payload: Option<&Value>, key: &str) -> Option<bool> {
    payload
        .and_then(|payload| payload.get("diag"))
        .and_then(|diag| diag.get(key))
        .and_then(Value::as_bool)
}
