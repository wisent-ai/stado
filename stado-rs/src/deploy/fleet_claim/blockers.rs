//! The per-host half: the reasons one declared host cannot claim, the two
//! spellings of the queue agent it may have declared, and the beacon read
//! that says whether that agent is running.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::deploy::{host_gates, service, DeployError};
use crate::models::Job;
use crate::monitor::host_health;
use crate::queue::capacity::Publication;
use crate::queue::JobStorage;
use crate::targets::{ComputeTarget, Registry};

use super::{wait_words, Blocker};

/// The `com.wisent.compute.agent.` label prefix
/// [`crate::deploy::local_install::label`] mints for `kind=agent`.
const MINTED_AGENT_PREFIX: &str = "com.wisent.compute.agent.";

/// The spelling an operator gets when they deploy a queue agent through
/// `stado service deploy`, which mints the `service` kind instead: the mini's
/// agent is declared as `com.wisent.compute.service.stado-agent-mini`.
const DEPLOYED_AGENT_MARK: &str = "stado-agent";

/// Every reason one host cannot claim, silence first and policy last: an
/// operator reads the top line to learn whether the host is talking at all,
/// and everything below it is only meaningful once it is.
pub(super) async fn host_blockers(
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

/// One `diag` boolean the agent published, read exactly as
/// [`host_gates`] reads it.
fn diag_flag(payload: Option<&Value>, key: &str) -> Option<bool> {
    payload
        .and_then(|payload| payload.get("diag"))
        .and_then(|diag| diag.get(key))
        .and_then(Value::as_bool)
}
