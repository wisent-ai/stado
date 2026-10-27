//! `stado contributions pending|report [--ledger URL] [--json]`: the time the
//! fleet's hosts spent running its jobs, reported to Wisent's contribution
//! ledger in compute.wisent.com as `compute.shared` contributions of each
//! host's owner.
//!
//! Every completed job that states when it started, when it completed and
//! which host ran it is one event: the job id is the event id, the quantity
//! is the seconds between start and completion, measured from the job's own
//! record, and the contributor is the Wisent account email the fleet
//! registry declares as that host's `contribution_owner`. The owner links
//! that address on the Rewards page by the account's verified email, so no
//! one can claim a host by a name they set: neither a typed host nor a
//! worker name or hostname decides who is credited. A completed job missing
//! its start, completion or host, or run on a host the registry lists without
//! an owner, is named as unmeasured with why, never given a value nobody
//! recorded.
//!
//! The ledger is asked once which of the events it already holds: `pending`
//! marks each event held or not when a ledger is named, and `report` sends
//! only the ones it does not hold, so a run costs one request per new job.

mod ledger;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::cli::reporting::field;
use crate::cli::CmdError;
use ledger::{Ledger, LEDGER_ENV};

const KIND: &str = "compute.shared";
const UNIT: &str = "second";
/// The lifecycle prefix completed jobs are kept under.
const COMPLETED: &str = "completed";

#[derive(Subcommand, Debug)]
pub enum ContributionsCommands {
    /// List the events report would send and the jobs it cannot measure; with a ledger, which it already holds.
    Pending(LedgerArgs),
    /// Send the events the contribution ledger does not hold yet and print what it answered.
    Report(LedgerArgs),
}

/// The registry target field naming the Wisent account email a host's shared
/// compute is credited to, declared by the fleet's operator.
const OWNER_FIELD: &str = "contribution_owner";

/// The account email the registry declares for `host`, or why it names none.
fn owner(registry: &crate::targets::Registry, host: &str) -> Result<String, String> {
    let Some(target) = registry.lookup(host) else {
        return Err(format!("the registry lists no host {host}"));
    };
    match target.extra.get(OWNER_FIELD) {
        Some(Value::String(owner)) if !owner.trim().is_empty() => Ok(owner.trim().to_string()),
        Some(other) => Err(format!(
            "host {host}'s {OWNER_FIELD} is not an address: {other}"
        )),
        None => Err(format!(
            "the registry names no {OWNER_FIELD} for host {host}; its operator declares one with \
             stado registry set --path targets.{host}.{OWNER_FIELD} --value <email>"
        )),
    }
}

#[derive(Args, Debug)]
pub struct LedgerArgs {
    /// The ledger's origin; without it, STADO_CONTRIBUTION_LEDGER_URL.
    #[arg(long)]
    ledger: Option<String>,
    #[arg(long)]
    json: bool,
}

fn seconds_between(started: &str, completed: &str) -> Result<i64, String> {
    let parse = |text: &str| {
        chrono::DateTime::parse_from_rfc3339(text)
            .map_err(|error| format!("{text:?} is not an RFC 3339 time: {error}"))
    };
    let span = parse(completed)? - parse(started)?;
    match span.num_seconds() {
        seconds if seconds.is_positive() => Ok(seconds),
        seconds => Err(format!(
            "it completed {seconds} seconds after it started, which measures no work"
        )),
    }
}

/// Every completed job as the event the ledger records, and the ones it
/// cannot measure or credit, with why.
async fn shared_compute() -> Result<(Vec<Value>, Vec<Value>), CmdError> {
    let store = crate::queue::JobStorage::new()
        .await
        .map_err(CmdError::from)?;
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(CmdError::from)?;
    let mut events = Vec::new();
    let mut unmeasured = Vec::new();
    for job in store.list_jobs(COMPLETED).await.map_err(CmdError::from)? {
        let host = job
            .worker_allocation
            .as_ref()
            .map(|allocation| allocation.host.clone());
        let measured = match (job.started_at.as_deref(), job.completed_at.as_deref(), host) {
            (Some(started), Some(completed), Some(host)) if !host.is_empty() => {
                seconds_between(started, completed).and_then(|seconds| {
                    owner(&registry, &host)
                        .map(|owner| (host, owner, seconds, completed.to_string()))
                })
            }
            (None, _, _) => Err("its record states no start".to_string()),
            (_, None, _) => Err("its record states no completion".to_string()),
            _ => Err("its record names no host that ran it".to_string()),
        };
        match measured {
            Ok((host, owner, seconds, completed)) => events.push(json!({
                "source_event_id": job.job_id,
                "kind": KIND,
                "contributor_id": owner,
                "quantity": seconds,
                "unit": UNIT,
                "evidence": {"job": job.job_id, "host": host, "started_at": job.started_at, "completed_at": completed},
                "occurred_at": completed,
            })),
            Err(reason) => unmeasured.push(json!({"job": job.job_id, "reason": reason})),
        }
    }
    Ok((events, unmeasured))
}

/// The ids of `events`, to ask the ledger about.
fn ids(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| event["source_event_id"].as_str())
        .collect()
}

fn print(value: &Value, json: bool, lines: impl Iterator<Item = String>) -> Result<(), CmdError> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        lines.for_each(|line| println!("{line}"));
    }
    Ok(())
}

async fn pending(args: LedgerArgs) -> Result<(), CmdError> {
    let (mut events, unmeasured) = shared_compute().await?;
    let url = ledger::named(args.ledger.as_deref())?;
    if let Some(url) = &url {
        let held = Ledger::open(url.clone()).await?.held(&ids(&events)).await?;
        for event in &mut events {
            let id = event["source_event_id"].as_str().map(str::to_string);
            event["held"] = Value::from(id.is_some_and(|id| held.contains(&id)));
        }
    }
    let lines = events
        .iter()
        .map(|event| {
            let state = match event.get("held").and_then(Value::as_bool) {
                Some(true) => " (held)",
                Some(false) => " (not held)",
                None => "",
            };
            format!(
                "{} {} {}s{state}",
                field(&event["source_event_id"]), field(&event["contributor_id"]), field(&event["quantity"])
            )
        })
        .chain(
            unmeasured
                .iter()
                .map(|job| format!("{} not reported: {}", field(&job["job"]), field(&job["reason"]))),
        );
    print(
        &json!({"ledger": url, "events": events, "unmeasured": unmeasured}),
        args.json,
        lines,
    )
}

async fn report(args: LedgerArgs) -> Result<(), CmdError> {
    let url = ledger::named(args.ledger.as_deref())?.ok_or_else(|| {
        CmdError::usage(format!(
            "no contribution ledger is named; pass --ledger URL or set {LEDGER_ENV}"
        ))
    })?;
    let ledger = Ledger::open(url).await?;
    let (events, unmeasured) = shared_compute().await?;
    let held = ledger.held(&ids(&events)).await?;
    let mut outcomes = Vec::new();
    let mut refused = Vec::new();
    for event in &events {
        let id = &event["source_event_id"];
        if id.as_str().is_some_and(|id| held.contains(id)) {
            continue;
        }
        match ledger.record(event).await {
            Ok(answer) => outcomes.push(json!({"job": id, "answer": answer})),
            Err(cause) => {
                refused.push(format!("{}: {cause}", field(id)));
                outcomes.push(json!({"job": id, "refused": cause}));
            }
        }
    }
    let lines = outcomes
        .iter()
        .map(
            |outcome| match outcome.get("refused").and_then(Value::as_str) {
                Some(cause) => format!("{} refused: {cause}", field(&outcome["job"])),
                None => format!(
                    "{} recorded: {}",
                    field(&outcome["job"]), field(&outcome["answer"]["recorded"])
                ),
            },
        )
        .chain(std::iter::once(format!(
            "{} already held by the ledger",
            held.len()
        )));
    let answer =
        json!({"ledger": ledger.url, "events": outcomes, "held": held, "unmeasured": unmeasured});
    print(&answer, args.json, lines)?;
    if refused.is_empty() {
        Ok(())
    } else {
        Err(CmdError::refused(format!(
            "the contribution ledger refused {}",
            refused.join("; ")
        )))
    }
}

pub async fn dispatch(command: ContributionsCommands) -> Result<(), CmdError> {
    match command {
        ContributionsCommands::Pending(args) => pending(args).await,
        ContributionsCommands::Report(args) => report(args).await,
    }
}
