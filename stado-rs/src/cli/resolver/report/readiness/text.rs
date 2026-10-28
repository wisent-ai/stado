//! The text form of `stado resolver status`: one line per fact, then one per
//! blocker.

use serde_json::Value;

use crate::cli::resolver::report::probe::AuthorityAnswer;
use crate::cli::resolver::report::vault::KeyringLockRepair;
use crate::service_resolution::ResolverAdapter;

/// What a bind probe said, in the word the text form uses.
fn listening_word(listening: Option<bool>) -> &'static str {
    match listening {
        Some(true) => "listening",
        Some(false) => "not-listening",
        None => "not-probed",
    }
}

/// Print the resolver's state, its process, its API and adapters, the
/// authority, the vault's keyring-lock repair, the serve stops and every
/// blocker, all read from `report` as `--json` prints it.
#[allow(clippy::too_many_arguments)]
pub(super) fn print(
    target: &str,
    resolver_state: &str,
    verdict: &str,
    generation: &str,
    stale: bool,
    api_bind: &str,
    api_listening: Option<bool>,
    probed: &[(&ResolverAdapter, Option<bool>)],
    authority_target: &str,
    authority: &AuthorityAnswer,
    repair: Option<&KeyringLockRepair>,
    report: &Value,
    blockers: &[String],
) {
    println!(
        "resolver {target} state={resolver_state} verdict={verdict} generation={generation} \
         stale={}",
        if stale { "yes" } else { "no" }
    );
    let process = &report["process"];
    match (
        process["cpuTime"].as_str(),
        process["elapsed"].as_str(),
        process["why"].as_str(),
    ) {
        (Some(cpu), Some(elapsed), _) => {
            println!(
                "process pid={} cpu={cpu} elapsed={elapsed}",
                crate::cli::reporting::field(&process["pid"])
            )
        }
        (_, _, Some(why)) => println!("process not read: {why}"),
        _ => println!("process not read: the report names no reading"),
    }
    println!("api {api_bind} {}", listening_word(api_listening));
    for (adapter, listening) in probed {
        println!(
            "adapter {} consumer={} {} {}",
            adapter.service,
            adapter.consumer,
            adapter.bind,
            listening_word(*listening)
        );
    }
    let authority_generation = match &authority.generation {
        Some(published) => published.to_string(),
        None => "-".to_string(),
    };
    let answered_after = match authority.answered_after_seconds {
        Some(seconds) => format!(" answered_after={seconds}s"),
        None => String::new(),
    };
    println!(
        "authority {authority_target} source={} {} generation={authority_generation}{answered_after}",
        authority.source,
        if authority.reachable {
            "reachable"
        } else {
            "unreachable"
        },
    );
    let remembered = &report["authority"]["remembered"];
    if let Some(standing) = remembered["unanswered"].as_str() {
        println!("authority not asked: {standing}");
    }
    let kept = &remembered["kept_snapshot"];
    if let (Some(version), Some(until)) = (kept["version"].as_str(), kept["until"].as_str()) {
        println!("authority snapshot kept version={version} until={until}");
    }
    if let Some(repair) = repair {
        println!(
            "vault keyring-lock-repair holder={} started={} outcome={}",
            repair.holder,
            repair.started,
            match &repair.outcome {
                Some(outcome) => outcome.as_str(),
                None => "running",
            }
        );
    }
    let serve_stops = &report["serve_stops"];
    if let Some(cause) = serve_stops["unreadable"].as_str() {
        println!("serve stops unreadable: {cause}");
    } else if !serve_stops.is_null() {
        let text = |key: &str| match serve_stops[key].as_str() {
            Some(value) => value.to_string(),
            None => format!("(no {key} reported)"),
        };
        // The Oko defect the stop was recorded as, when one was.
        let defect = match &serve_stops["report"] {
            serde_json::Value::Null => "none recorded".to_string(),
            serde_json::Value::String(report) => report.clone(),
            other => other.to_string(),
        };
        println!(
            "serve stops stado {} (revision {}) stopped {} time(s), first {}, latest {}: {}; \
             defect: {defect}",
            text("version"),
            text("revision"),
            crate::cli::reporting::field(&serve_stops["stops"]),
            text("first_at"),
            text("latest_at"),
            text("latest_failure"),
        );
        if serve_stops["held"] == true {
            println!(
                "serve stops revision {} is held from source installs until the checkout moves \
                 past it",
                text("revision")
            );
        }
    }
    for blocker in blockers {
        println!("blocker {blocker}");
    }
}
