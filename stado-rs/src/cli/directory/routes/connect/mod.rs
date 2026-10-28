//! `stado service directory connect` — the address a caller should dial for a
//! service, derived from where the directory places it and who is asking.
//!
//! Every answer read from the directory says which service directory
//! generation decided it: `generation` in the `--json` answer, and in the
//! `details` of a refusal given after the document was read. A caller that
//! keeps an answer (Oko keeps a route, and a refusal Stado marks as not
//! retryable) holds it while this machine's resolver publishes that same
//! generation and asks again once it publishes another; without the
//! generation it could only guess when the answer stopped being true.

mod adapter;
mod generation;

use serde_json::{json, Value};

use crate::observations;

use crate::cli::CmdError;

use crate::cli::directory::document::{
    directory, endpoint_url, named_target_in, read_document, service, this_target_in,
};
use crate::cli::directory::routes::{answers, service_port};
use adapter::{adapter_route, route_failure};
use generation::{generation_of, under};

pub(in crate::cli::directory) async fn connect(
    name: &str,
    target: Option<String>,
    consumer: Option<String>,
    no_verify: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    let document = read_document().await?;
    let generation = generation_of(&document);
    answer(
        &document, generation, name, target, consumer, no_verify, as_json,
    )
    .await
    .map_err(|error| under(error, generation))
}

/// The scheme of `name`'s endpoint on its active host, for a service that
/// declares no `scheme` of its own: the address its own host is declared
/// at says how it is spoken to.
fn endpoint_scheme(entry: &Value, name: &str, active: &str) -> Result<String, CmdError> {
    let Some(address) = endpoint_url(entry, active) else {
        return Err(CmdError::declaration(format!(
            "{name} declares no scheme and no endpoint on {active} to read one from; \
             declare `scheme` on the service"
        )));
    };
    url::Url::parse(address.trim())
        .map(|parsed| parsed.scheme().to_string())
        .map_err(|error| {
            CmdError::declaration(format!(
                "{name} declares no scheme, and its endpoint on {active} ({address}) is not a URL \
                 to read one from: {error}"
            ))
        })
}

async fn answer(
    document: &Value,
    generation: Option<u64>,
    name: &str,
    target: Option<String>,
    consumer: Option<String>,
    no_verify: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // Consumer admission and placement fencing are the same as the fleet
    // resolver's, including when the caller asks for an unverified address.
    if let Some(consumer) = consumer.as_deref() {
        crate::service_resolution::resolve(document, name, consumer).map_err(|error| {
            use crate::service_resolution::ResolveError;
            let state = match &error {
                ResolveError::UnknownService(_) => "undeclared",
                ResolveError::Declaration(_) => "invalid_declaration",
                ResolveError::Unauthorized { .. } => "refused",
                ResolveError::Moving { .. } => "unavailable",
            };
            route_failure(CmdError::from(error), name, Some(consumer), state)
        })?;
    }
    let block = directory(document)?;
    let entry = service(block, name)?;
    let asking = match target {
        Some(value) => value,
        None => this_target_in(document)?,
    };
    let invalid = |error| route_failure(error, name, consumer.as_deref(), "invalid_declaration");
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "{name} declares no active_host, so there is no placement to route to"
            ))
        })
        .map_err(invalid)?;
    let port = service_port(entry, active)
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "{name} is placed on {active} but declares no port, and none can be read \
             back from an address for that host"
            ))
        })
        .map_err(invalid)?;
    let scheme = match entry
        .get("scheme")
        .and_then(Value::as_str)
        .filter(|scheme| !scheme.is_empty())
    {
        Some(declared) => declared.to_string(),
        None => endpoint_scheme(entry, name, active).map_err(invalid)?,
    };

    // A service that is not here is reached through THIS machine's own resolver
    // adapter, because that is the only address on this machine that leads to
    // it. These services bind loopback on their own host by design, so
    // `scheme://<that host's address>:<its port>` names a socket nobody outside
    // that host can open, so answering with it hands the caller an address
    // that fails. `ARCHITECTURE.md` states the rule
    // this broke: a client "must look up its own target rather than reconstruct
    // an address from a host name".
    //
    // The lookup is `registry.targets[<asking>].service_resolver.adapters`,
    // which already declares one loopback bind per (service, consumer) pair on
    // every machine that consumes a service. Nothing new is declared here; the
    // declaration was simply never read, so every client that needed a working
    // address grew a hand-written pointer file beside it, with a loopback
    // port typed into it.
    //
    // `--consumer` selects among a machine's adapters for the same service. One
    // adapter needs no choosing; several without a named consumer is ambiguous
    // and says so, because picking one silently is how a caller ends up on
    // another program's channel.
    // A service this host runs for its own callers beside its placement
    // (`stado service directory beside`) is answered here, said as such; only
    // this host's own view reads that statement. A machine the registry does
    // not name has no own view to compute; a registry that cannot say which
    // machine this is refuses the answer with its cause.
    let here = named_target_in(document)?;
    let own_view = here.as_deref() == Some(asking.as_str());
    let beside = match own_view {
        // This host's own statement decides it; a refusal here states no
        // directory generation, so no caller keeps it past the statement.
        true => crate::cli::directory::beside::local_url(name).map_err(generation::local)?,
        false => None,
    };
    let placed_on = match beside {
        Some(_) => asking.clone(),
        None => active.to_string(),
    };
    let through_adapter = beside.is_none() && asking != active;
    let url = if let Some(local) = beside.clone() {
        local
    } else if !through_adapter {
        format!("{scheme}://{}:{port}", std::net::Ipv4Addr::LOCALHOST)
    } else {
        match adapter_route(document, &asking, name, consumer.as_deref(), &scheme)? {
            Some(route) => route,
            None => {
                endpoint_url(entry, &asking)
                    .filter(|url| !url.trim().is_empty())
                    .ok_or_else(|| {
                        CmdError::declaration(format!(
                            "{name} is placed on {active}, but {asking} declares no caller endpoint \
                             or resolver adapter for consumer {consumer:?}; declare the route with \
                             `stado service directory consumer add {name} <consumer> --target {asking}`; \
                             the serving host's address is not a caller route"
                        ))
                    }).map_err(invalid)?
                    .to_string()
            }
        }
    };

    // Verification happens from this process, so it can only speak for this
    // machine. Asked to compute another target's view, the honest answer is the
    // address and an admission that nobody checked it -- probing anyway would
    // knock on this host's own loopback and report the result as if it came
    // from somewhere else, which is the confusion this command exists to end.
    let (placement, standing) = match beside {
        Some(_) => (
            "beside",
            format!("runs beside its placement on {placed_on}"),
        ),
        None => ("placed", format!("is placed on {placed_on}")),
    };
    let probe = if no_verify || !own_view {
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
        let fact = observations::service_fact(name, &asking);
        let row = observations::Observation::now(fact, asking.as_str(), state, detail);
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
        // not answer -- not some other address that happens to be up. When
        // the address is this host's own resolver adapter and that resolver
        // published why it holds no listener, the refusal carries that
        // reason: the socket nobody opened is the resolver's, not the
        // service's, and a reader met with "connection refused" alone went
        // looking for the service on the wrong host.
        Some(Err(detail)) => {
            let refused = format!("{name} {standing} and did not answer at {url}: {detail}");
            let sentence =
                match through_adapter.then(|| crate::cli::resolver::not_serving_reason(&url)) {
                    Some(Some(reason)) => format!("{refused}; {reason}"),
                    _ => refused,
                };
            return Err(CmdError::click(sentence)
                .stating(crate::primitives::failure::FailureCode::InfraDown));
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
                "placed_on": placed_on,
                "placement": placement,
                "from": asking,
                "consumer": consumer,
                "url": url,
                "verified": status.is_some(),
                "status": status,
                "checked_from": own_view.then(|| asking.clone()),
                "observed": observed,
                "generation": generation,
            }))?
        );
    } else {
        match status {
            Some(status) => {
                println!("{url}  ({name} {standing}, answered {status}, observed {observed})")
            }
            None if !own_view => println!(
                "{url}  ({name} {standing}, computed for {asking}, not checked from here, \
                 observed {observed})"
            ),
            None => println!("{url}  ({name} {standing}, unverified, observed {observed})"),
        }
    }
    Ok(())
}
