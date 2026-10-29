//! Where placed work runs: the judgement of fleet hosts, read from what each
//! host's own agent published about itself.
//!
//! A host is offered work only while its capacity publication is live (within
//! the next publication its author promised), its Stado runs the work
//! (`placed_work`), and it runs no CI job (`ci_jobs_running`, counted from its
//! process table): a CI runner builds for CI while it runs, and nothing is
//! put beside that. A host that accepts placements now is admitted; one that
//! refuses only because it is busy (its CPU, its RAM headroom, an exclusive
//! job, its janitor's turn, a CI job) is waiting and still takes the work
//! when it frees; any other refusal holds it back. Among hosts alike, a
//! machine whose registry role is `interactive` — the one an operator works
//! at — comes after every other, then more free cores before fewer. Every
//! host's verdict is returned, so a placement record names where the work
//! went and why every other host was passed over.

use chrono::Utc;
use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::queue::capacity::{publication_of, read_publications, Publication};
use crate::targets::{ComputeTarget, Registry};

/// The placed work this build of Stado runs when a placement pins it to the
/// host it runs on; every capacity publication states it (`placed_work`).
pub const PLACED_WORK: &[&str] = &["quality-check"];

/// The registry role of a machine an operator works at: work goes there only
/// when no other host can take it.
const INTERACTIVE_ROLE: &str = "interactive";

/// The admission reasons a host's agent publishes while it is only busy:
/// the claim gate takes the work once they clear. The same reasons a release
/// build waits on (`release_submit::builds::builder`), and the agent's own
/// reservation ceiling.
const BUSY: &[&str] = &[
    "cpu_busy",
    "ram_headroom_low",
    "exclusive_job_running",
    "reservations_exhausted",
    crate::providers::local::space_reclaim::CLEANUP_IN_PROGRESS,
];

/// One host's answer to one placement.
pub enum Standing {
    /// Takes the work now.
    Admitted,
    /// Takes the work once the named load clears.
    Waiting(String),
    /// Cannot take the work, for the named reason.
    Refused(String),
}

pub struct Verdict {
    pub target: ComputeTarget,
    pub standing: Standing,
    pub free_cores: Option<i64>,
    pub interactive: bool,
}

impl Verdict {
    fn rank(&self) -> u8 {
        match self.standing {
            Standing::Admitted => 0,
            Standing::Waiting(_) => 1,
            Standing::Refused(_) => 2,
        }
    }

    pub fn to_json(&self) -> Value {
        let (standing, reason) = match &self.standing {
            Standing::Admitted => ("admitted", None),
            Standing::Waiting(reason) => ("waiting", Some(reason)),
            Standing::Refused(reason) => ("refused", Some(reason)),
        };
        json!({
            "target": self.target.name,
            "standing": standing,
            "reason": reason,
            "free_cpu_cores": self.free_cores,
            "role": self.target.role,
        })
    }

    /// One line: the host, its standing and why.
    pub fn describe(&self) -> String {
        match &self.standing {
            Standing::Admitted => format!("{} admitted", self.target.name),
            Standing::Waiting(reason) => format!("{} waiting: {reason}", self.target.name),
            Standing::Refused(reason) => format!("{} refused: {reason}", self.target.name),
        }
    }
}

/// The hosts a placement may use: `requested` alone when named, else every
/// local host of the canonical registry this machine can reach. Without the
/// canonical registry only this host is known, and a named other host is
/// refused.
pub async fn fleet_hosts(
    requested: Option<&str>,
) -> Result<(Registry, Vec<ComputeTarget>), CmdError> {
    let (registry, canonical) = match crate::targets::fetch_registry_or_last_good().await {
        Ok((registry, notice)) => {
            if let Some(notice) = notice {
                crate::targets::report_registry_notice(&notice);
            }
            let canonical = registry.staleness_seconds.is_none();
            (registry, canonical)
        }
        Err(error) => {
            eprintln!(
                "stado: the canonical registry cannot be read ({error}); placing on this host \
                 from the registry built into stado"
            );
            (
                crate::targets::load_bundled_registry().map_err(CmdError::from)?,
                false,
            )
        }
    };
    if let Some(name) = requested {
        let target = host_channel::resolve_target(&registry, name)
            .map_err(CmdError::from)?
            .clone();
        if !canonical && !host_channel::target_is_this_host(&target) {
            return Err(CmdError::click(
                "the canonical registry is unavailable; refresh it before placing on another host",
            )
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        return Ok((registry, vec![target]));
    }
    let targets = registry
        .targets
        .iter()
        .filter(|target| {
            target.is_provider(crate::capabilities::ProviderId::Local)
                && (host_channel::target_is_this_host(target)
                    || (canonical && target.has_ssh_connection()))
        })
        .cloned()
        .collect();
    Ok((registry, targets))
}

/// Every host of `targets` judged for `work`, best first. A store that cannot
/// be read is refused: read as no publications, every host would look silent
/// and the work would be placed on none.
pub async fn judge(
    registry: &Registry,
    targets: Vec<ComputeTarget>,
    work: &str,
) -> Result<Vec<Verdict>, CmdError> {
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| CmdError::from(error).within("open the fleet store for placement"))?;
    let publications = read_publications(&store)
        .await
        .map_err(|error| CmdError::from(error).within("read the hosts' capacity publications"))?;
    let mut verdicts = Vec::new();
    for target in targets {
        let publication = publication_of(registry, &target, &publications)
            .map_err(CmdError::from)?
            .map(|(_, publication)| publication);
        let free_cores = publication
            .and_then(|publication| publication.payload.get("available_cpu_cores"))
            .and_then(Value::as_i64);
        let standing = standing(&target.name, publication, work);
        verdicts.push(Verdict {
            interactive: target.role.as_deref() == Some(INTERACTIVE_ROLE),
            target,
            standing,
            free_cores,
        });
    }
    verdicts.sort_by(|left, right| {
        left.rank()
            .cmp(&right.rank())
            .then_with(|| left.interactive.cmp(&right.interactive))
            .then_with(|| right.free_cores.cmp(&left.free_cores))
            .then_with(|| left.target.name.cmp(&right.target.name))
    });
    Ok(verdicts)
}

/// Where `host` stands for `work`, in its own publication's words.
fn standing(host: &str, publication: Option<&Publication>, work: &str) -> Standing {
    let Some(publication) = publication else {
        return Standing::Refused(format!(
            "{host} publishes no capacity, so whether it can run {work} is not known"
        ));
    };
    if publication.stale(Utc::now()) {
        let promised = match publication.next_by() {
            Some(next) => format!("its next was promised by {next}"),
            None => "it promised no next one".to_string(),
        };
        let published = match publication.stamp {
            Some(stamp) => format!("published {stamp}"),
            None => "undated".to_string(),
        };
        return Standing::Refused(format!(
            "{host}'s last capacity publication is stale ({published}; {promised}), so what it \
             has free now is not known"
        ));
    }
    let payload = &publication.payload;
    let runs = payload
        .get("placed_work")
        .and_then(Value::as_array)
        .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some(work)));
    if !runs {
        let version = payload
            .get("stado_version")
            .and_then(Value::as_str)
            .unwrap_or("of no stated version");
        return Standing::Refused(format!(
            "{host}'s Stado {version} does not run placed {work}; install a Stado that does with \
             `stado product install stado --surface service --host {host}`"
        ));
    }
    match payload.get("ci_jobs_running").and_then(Value::as_u64) {
        Some(0) => {}
        Some(running) => {
            return Standing::Waiting(format!(
                "{host} is running {running} CI job(s), and nothing is placed beside a CI run"
            ))
        }
        None => {
            let cause = payload
                .get("ci_jobs_unread")
                .and_then(Value::as_str)
                .unwrap_or("no cause stated");
            return Standing::Refused(format!(
                "{host} could not count its CI jobs ({cause}), so whether it is building for CI is \
                 not known"
            ));
        }
    }
    let reason = payload
        .get("diag")
        .and_then(|diag| diag.get("admission_reason"))
        .and_then(Value::as_str);
    match (
        payload.get("accepting_jobs").and_then(Value::as_bool),
        reason,
    ) {
        (Some(true), _) => Standing::Admitted,
        (Some(false), Some(reason)) if BUSY.contains(&reason) => {
            Standing::Waiting(format!("{host} is busy ({reason})"))
        }
        (Some(false), reason) => Standing::Refused(format!(
            "{host} is not accepting placements ({})",
            reason.unwrap_or("its agent published no reason")
        )),
        (None, _) => Standing::Refused(format!(
            "{host}'s capacity publication names no accepting_jobs, so whether it accepts \
             placements is not known"
        )),
    }
}

/// Every verdict as a placement record carries it.
pub fn record(verdicts: &[Verdict]) -> Value {
    Value::Array(verdicts.iter().map(Verdict::to_json).collect())
}

/// What credit-funded capacity says when no fleet host can take `work`: the
/// Azure credit gate's decision on the balance read now, as `stado billing
/// show` states it (`provision_gate`). Placed work rents no machine yet; the
/// sentence says whether the credits would admit one and, when not, the
/// gate's own refusal (the floor ruling not recorded, a balance unread, a
/// balance at or below the floor).
pub async fn credit_capacity(work: &str) -> String {
    let section = crate::monitor::billing::azure_balance_section().await;
    let gate = crate::providers::azure::credit::standing(&section).await;
    match (gate["admits"].as_bool(), gate["refusal"].as_str()) {
        (Some(true), _) => format!(
            "the Azure credits are above the operator's floor, but placed {work} rents no cloud \
             machine; it waits for a fleet host"
        ),
        (_, Some(refusal)) => format!("no Azure machine is rented for it: {refusal}"),
        _ => {
            format!("no Azure machine is rented for it: the credit gate gave no decision ({gate})")
        }
    }
}
