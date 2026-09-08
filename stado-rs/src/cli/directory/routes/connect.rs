//! `stado service directory connect` — the address a caller should dial for a
//! service, derived from where the directory places it and who is asking.

use serde_json::{json, Value};

use crate::observations;

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::directory::document::{click, directory, service, this_target};
use crate::cli::directory::routes::{answers, routable_address, service_port};

/// The loopback address `asking` declares for reaching `service`, or `None` when
/// that machine declares no adapter for it.
///
/// Read from `registry.targets[<asking>].service_resolver.adapters`, the same
/// declaration the resolver itself binds. A machine that consumes a service
/// through the resolver has exactly one bind per consumer, so naming the
/// consumer is what disambiguates two programs sharing a service: `brama` on an
/// operator laptop is bound once for `operator` and once for `brama-desktop`,
/// and handing the wrong one to a caller puts it on a channel whose idle and
/// connect budgets belong to somebody else.
fn adapter_route(
    document: &Value,
    asking: &str,
    service: &str,
    consumer: Option<&str>,
    scheme: &str,
) -> Result<Option<String>, CmdError> {
    let Ok(config) = crate::service_resolution::resolver_config(document, asking) else {
        return Ok(None);
    };
    let mut declared: Vec<&crate::service_resolution::ResolverAdapter> = config
        .adapters
        .iter()
        .filter(|adapter| adapter.service == service)
        .collect();
    if let Some(consumer) = consumer {
        declared.retain(|adapter| adapter.consumer == consumer);
        if declared.is_empty() {
            return Err(click(format!(
                "{asking} declares no resolver adapter for {service} as consumer {consumer}; \
                 declare one in registry.targets[{asking}].service_resolver.adapters"
            )));
        }
    }
    match declared.as_slice() {
        [] => Ok(None),
        [adapter] => Ok(Some(format!("{scheme}://{}", adapter.bind))),
        several => Err(click(format!(
            "{asking} declares {} resolver adapters for {service}, one per consumer ({}); \
             name the caller with --consumer",
            several.len(),
            several
                .iter()
                .map(|adapter| adapter.consumer.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub(in crate::cli::directory) async fn connect(
    name: &str,
    target: Option<String>,
    consumer: Option<String>,
    no_verify: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    let entry = service(block, name)?;
    let asking = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            click(format!(
                "{name} declares no active_host, so there is no placement to route to"
            ))
        })?;
    let port = service_port(entry, active).ok_or_else(|| {
        click(format!(
            "{name} is placed on {active} but declares no port, and none can be read \
             back from an address for that host"
        ))
    })?;
    let scheme = entry
        .get("scheme")
        .and_then(Value::as_str)
        .filter(|scheme| !scheme.is_empty())
        .unwrap_or("http");

    // A service that is not here is reached through THIS machine's own resolver
    // adapter, because that is the only address on this machine that leads to
    // it. These services bind loopback on their own host by design, so
    // `scheme://<that host's address>:<its port>` names a socket nobody outside
    // that host can open -- and this verb answered exactly that until
    // 2026-09-04, when `connect brama` from lukasz-macbook returned
    // `http://100.120.25.24:8080` and failed. `ARCHITECTURE.md` states the rule
    // this broke: a client "must look up its own target rather than reconstruct
    // an address from a host name".
    //
    // The lookup is `registry.targets[<asking>].service_resolver.adapters`,
    // which already declares one loopback bind per (service, consumer) pair on
    // every machine that consumes a service. Nothing new is declared here; the
    // declaration was simply never read, so every client that needed a working
    // address grew a hand-written pointer file beside it. Lem carried one for
    // months with `127.0.0.1:17621` typed into it.
    //
    // `--consumer` selects among a machine's adapters for the same service. One
    // adapter needs no choosing; several without a named consumer is ambiguous
    // and says so, because picking one silently is how a caller ends up on
    // another program's channel.
    let url = if asking == active {
        format!("{scheme}://127.0.0.1:{port}")
    } else {
        match adapter_route(&document, &asking, name, consumer.as_deref(), scheme)? {
            Some(route) => route,
            None => {
                let registry = registry::read_registry().await?;
                let placed = registry
                    .targets
                    .iter()
                    .find(|candidate| candidate.name == active)
                    .ok_or_else(|| {
                        click(format!(
                            "{name} is placed on {active}, which is not a host in the registry"
                        ))
                    })?;
                let address = routable_address(placed).ok_or_else(|| {
                    click(format!(
                        "{name} is placed on {active}, and that host's record carries no address \
                         reachable from {asking}"
                    ))
                })?;
                format!("{scheme}://{address}:{port}")
            }
        }
    };

    // Verification happens from this process, so it can only speak for this
    // machine. Asked to compute another target's view, the honest answer is the
    // address and an admission that nobody checked it -- probing anyway would
    // knock on this host's own loopback and report the result as if it came
    // from somewhere else, which is the confusion this command exists to end.
    let here = this_target().await.unwrap_or_default();
    let probe = if no_verify || asking != here {
        None
    } else {
        Some(answers(&url).await)
    };

    // A look that is not written down is a look that did not happen: the next
    // reader of the directory sees `never` and re-derives the same doubt. This
    // is the one verb in this file that actually knocks, so its result becomes
    // the fleet's record and not just one line of console output. Recorded
    // before the failure is raised, because `unreachable` is the state the
    // whole change exists to preserve -- returning the error first would throw
    // away the only evidence anyone has that somebody checked.
    if let Some(outcome) = probe.as_ref() {
        let (state, detail) = match outcome {
            Ok(status) => (observations::OBSERVED, format!("HTTP {status} at {url}")),
            Err(detail) => (observations::UNREACHABLE, format!("{url}: {detail}")),
        };
        let fact = observations::service_fact(name, &here);
        let row = observations::Observation::now(fact, here.as_str(), state, detail);
        // A record that cannot be written must not turn a successful connect
        // into a failure: the caller asked where the service is, and the
        // answer stands whether or not this host can keep notes.
        if let Err(error) = observations::record(&[row]) {
            eprintln!("warning: could not record the observation: {error}");
        }
    }

    let status = match probe {
        Some(Ok(status)) => Some(status),
        // Deliberately terminal. The caller asked where this service is,
        // and the honest answer is that it is placed somewhere that did
        // not answer -- not some other address that happens to be up.
        Some(Err(detail)) => {
            return Err(click(format!(
                "{name} is placed on {active} and did not answer at {url}: {detail}"
            )))
        }
        None => None,
    };

    // The vantage that matters is the one being computed for, not the one
    // running the command: `--target other-host` prints the address that host
    // is handed, so the age shown must be the age of that host's evidence.
    let observed = observations::describe(&observations::service_fact(name, &asking));

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "placed_on": active,
                "from": asking,
                "consumer": consumer,
                "url": url,
                "verified": status.is_some(),
                "status": status,
                "checked_from": if asking == here { Some(here.clone()) } else { None },
                "observed": observed,
            }))?
        );
    } else {
        match status {
            Some(status) => {
                println!("{url}  ({name} on {active}, answered {status}, observed {observed})")
            }
            None if asking != here => println!(
                "{url}  ({name} on {active}, computed for {asking}, not checked from here, \
                 observed {observed})"
            ),
            None => println!("{url}  ({name} on {active}, unverified, observed {observed})"),
        }
    }
    Ok(())
}
