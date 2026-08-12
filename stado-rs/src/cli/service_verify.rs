//! Does the service directory still describe the world, or only itself?
//!
//! Every other check in this binary compares Stado's declarations against each
//! other. `config validate` checks the document against a schema. `registry
//! validate` checks placement profiles against targets. `doctor` checks that a
//! backend the config names can be constructed. All of them can pass in full
//! while nothing the fleet declares is actually reachable, because not one of
//! them goes and looks.
//!
//! On 2026-08-11 that cost twelve days of a worker's output. The directory
//! declared `stado-object-api` active on a laptop. The service-directory schema
//! requires endpoints be host-relative loopback, so every other host reached it
//! through a forward. The laptop was closed, the forward had no upstream, and
//! the worker on the always-on Mac refused 29,616 times to claim work whose
//! diagnostics it could not upload. Every declaration involved was valid.
//! `config validate`, `registry validate` and `doctor` all passed throughout,
//! on both machines, because none of them was ever about reachability.
//!
//! `identity verify` already exists for exactly this reason, one aisle over: it
//! reads the host instead of trusting the binding, "because these identities are
//! granted elsewhere and revoked without notice". A service endpoint is granted
//! elsewhere and revoked without notice too -- by a lid closing. This module is
//! that same idea applied to the declarations the whole fleet routes through.
//!
//! Three states, never two:
//!
//!   observed     something answered at the declared endpoint, from the host
//!                that is told to call it. The declaration is true right now.
//!   unreachable  nothing answered. The declaration is false, and this is the
//!                state that hid for twelve days behind a passing validator.
//!   unverified   the probe could not run: host down, helper not installed,
//!                channel refused. Kept apart from `unreachable` deliberately --
//!                "I did not look" and "I looked and it is gone" send an
//!                operator to two different places, and collapsing them is how a
//!                fleet learns to ignore its own reports.
//!
//! The vantage is the point. A service is verified from each consumer's own
//! host, over the endpoint that consumer is handed, because that is the only
//! question with an operational answer. Probing from the serving host proves the
//! process is alive and proves nothing about whether the fleet can reach it --
//! which is precisely the gap this fleet fell into.
//!
//! What is probed, and from where, is no longer this file's decision. Every
//! declaration carries its own verification descriptor -- kind, vantage, what
//! counts as an answer -- and this command is the driver for it. The single
//! hardcoded probe was correct for every entry the directory holds today and
//! would have been wrong, silently and with a verdict, for the first entry
//! that was not an HTTP service: a database socket called `unreachable` while
//! serving, because the checker asked in a language the service does not
//! speak. A checker that answers questions it did not ask is the defect this
//! command was written to remove, not one it may commit.
//!
//! An entry that says nothing derives the default, which is precisely the
//! probe this file used to hardcode, so no existing declaration changes
//! verdict. A descriptor naming a kind or vantage this build does not
//! implement is `unverified` with the offending word in the detail, and
//! `targets::validate_verification` raises the same complaint against its
//! author when the registry is validated -- long before an operator has to
//! read it off a sweep.
//!
//! One ambiguity is worth stating, because it decides what this command calls a
//! failure and the data model does not settle it. `Service::endpoints` is keyed
//! by host, and two readings survive the type: "the address this host uses to
//! reach the service", and "where this host would serve it if the service moved
//! here". The field's own comment leans the second way -- a host carrying an
//! endpoint is not thereby serving -- while `service directory publish`
//! implements the first, writing `endpoints[<this host>]` into that host's
//! `~/.stado/forwards/<service>.local` for consumers on it to read.
//!
//! This follows `publish`, because that is the code consumers actually run. So
//! an endpoint that answers nothing from the host it is written for is reported
//! unreachable even when that host is only standing by: under the executable
//! reading, a standby host has been handed an address that does not work, and
//! the day it is promoted is the worst possible time to find out. If the other
//! reading is the intended one, the fix is in the model -- give a standby
//! address its own field -- not in this command, which would then be reporting
//! a real inconsistency between two parts of one system.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::cli::CmdError;
// The three state words are imported, never respelled here. This command
// writes them into the observation record and other commands read them back
// out of it, so a private copy that drifted by one letter would file rows
// nothing matches -- a fact with no reader, which is the defect this change
// exists to remove.
use crate::observations::{service_fact, Observation, OBSERVED, UNREACHABLE, UNVERIFIED};
use crate::targets::{
    load_registry_auto, Registry, Service, VerifyDescriptor, VERIFY_FROM_ACTIVE_HOST,
    VERIFY_FROM_ENDPOINT_HOLDERS, VERIFY_KIND_HTTP, VERIFY_KIND_TCP,
};

/// Helper that runs this same command in `--local` mode on a remote host.
/// Installed with
/// `stado host install-helper <target> stado-rs/scripts/probe-service-endpoints.sh
/// probe-service-endpoints`.
const PROBE_HELPER: &str = "probe-service-endpoints";

/// A probe must not hang a fleet sweep behind one dead forward. Long enough for
/// a loopback service under load, short enough that a closed laptop answers
/// promptly with the truth.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// One declaration, checked or explicitly not checked.
struct Finding {
    service: String,
    host: String,
    endpoint: String,
    state: &'static str,
    detail: String,
}

impl Finding {
    fn to_json(&self) -> Value {
        json!({
            "service": self.service,
            "host": self.host,
            "endpoint": self.endpoint,
            "state": self.state,
            "detail": self.detail,
        })
    }
}

/// Which hosts probe this service?
///
/// [`VERIFY_FROM_ENDPOINT_HOLDERS`], the default, is every host the directory
/// hands an address to, plus the active host. That map is what a consumer
/// actually reads: `service directory publish` writes `endpoints[<this host>]`
/// into `~/.stado/forwards/<service>.local` and skips a service that gives
/// this host no entry. So a host carrying an endpoint has been handed an
/// address and can be held to it, whether or not it is the one serving.
///
/// [`VERIFY_FROM_ACTIVE_HOST`] is only where the service claims to serve, for
/// an endpoint no other host was ever meant to reach. Probing that from four
/// vantages files four `unreachable` rows against a service working exactly as
/// declared, and a report that cries wolf gets read like one.
///
/// An unrecognized vantage keeps the wider set on purpose, so no declaration
/// vanishes from the table; those rows come back `unverified` naming the
/// vantage. A missing row reads as "fine" to every operator alive.
///
/// Deliberately NOT `consumers`. That map is keyed by consumer identity -- the
/// name of the software calling in, like `weles` -- and not by host, so reading
/// it as a host set produces confident answers about machines that do not
/// exist. The consumer-to-host mapping lives in placement, not here, and
/// guessing at it would put this command in the same class of defect it was
/// written to catch.
fn probe_hosts(service: &Service, descriptor: &VerifyDescriptor) -> BTreeSet<String> {
    let endpoint_holders = || {
        let mut hosts: BTreeSet<String> = service.endpoints.keys().cloned().collect();
        hosts.insert(service.active_host.clone());
        hosts
    };
    match descriptor.from.as_str() {
        VERIFY_FROM_ACTIVE_HOST => BTreeSet::from([service.active_host.clone()]),
        VERIFY_FROM_ENDPOINT_HOLDERS => endpoint_holders(),
        _ => endpoint_holders(),
    }
}

/// The descriptor asks for something this build cannot do, spelled out for the
/// row it will produce.
///
/// This is the registry validator's own function, deliberately. One list of
/// implemented values means a descriptor cannot pass validation and then find
/// no prober, nor be refused by a prober the validator was happy with -- two
/// lists is how a declaration ends up with a reader that does not exist.
fn unsupported(service: &str, descriptor: &VerifyDescriptor) -> Option<String> {
    let problems = crate::targets::validate_verification(service, descriptor);
    if problems.is_empty() {
        return None;
    }
    Some(problems.join("; "))
}

/// The address a given host is told to use.
///
/// The health path is appended for `http` and withheld for `tcp`: a path is
/// not something you can send down a socket, and pasting one onto an address
/// produces a port nobody is listening on. An endpoint with no entry for a
/// host that is supposed to call it is itself a finding: the consumer has been
/// authorized and given no address.
fn endpoint_for(service: &Service, host: &str, kind: &str) -> Option<String> {
    let endpoint = service.endpoints.get(host)?;
    if kind != VERIFY_KIND_HTTP {
        return Some(endpoint.url.clone());
    }
    let health = endpoint
        .extra
        .get("health")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if health.is_empty() {
        return Some(endpoint.url.clone());
    }
    Some(format!(
        "{}/{}",
        endpoint.url.trim_end_matches('/'),
        health.trim_start_matches('/')
    ))
}

/// Ask the endpoint whether anything is there, in the language the declaration
/// says it speaks.
///
/// The catch-all arm is `unverified`, and it is not dead code behind
/// [`unsupported`]: it is the guarantee that no path through this file can
/// turn a kind nobody implemented into a verdict about a service nobody
/// probed.
async fn probe(kind: &str, endpoint: &str) -> (&'static str, String) {
    match kind {
        VERIFY_KIND_HTTP => probe_http(endpoint).await,
        VERIFY_KIND_TCP => probe_tcp(endpoint).await,
        other => (
            UNVERIFIED,
            format!("no probe implemented for verification kind '{other}'"),
        ),
    }
}

/// Any HTTP response counts as observed, including 401, 404 and 503. This
/// verifies that the declaration points at something serving, not that the
/// service is healthy: a 503 from a real server is a different and much
/// smaller problem than a connection that goes nowhere, and conflating them is
/// what let `fetch failed` sit in a log for twelve days looking like an
/// application bug.
async fn probe_http(url: &str) -> (&'static str, String) {
    let client = match reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => return (UNVERIFIED, format!("no HTTP client: {error}")),
    };
    match client.get(url).send().await {
        Ok(response) => (OBSERVED, format!("HTTP {}", response.status().as_u16())),
        Err(error) if error.is_timeout() => (
            UNREACHABLE,
            format!("no answer within {}s", PROBE_TIMEOUT.as_secs()),
        ),
        Err(error) => (UNREACHABLE, root_cause(&error)),
    }
}

/// Connect, then hang up.
///
/// For an endpoint that speaks no HTTP: a database socket, a line protocol, a
/// port whose first byte is the server's. A completed handshake is the whole
/// of what such a declaration promises -- something is accepting on the
/// address the directory hands out -- and nothing is sent, because a probe
/// that guessed at the protocol would at best hang in someone's parser and at
/// worst be a write.
async fn probe_tcp(endpoint: &str) -> (&'static str, String) {
    let Some(address) = socket_address(endpoint) else {
        return (
            UNVERIFIED,
            format!("endpoint is not a host:port address: {endpoint}"),
        );
    };
    match tokio::time::timeout(
        PROBE_TIMEOUT,
        tokio::net::TcpStream::connect(address.as_str()),
    )
    .await
    {
        Ok(Ok(_stream)) => (OBSERVED, format!("connected to {address}")),
        Ok(Err(error)) => (UNREACHABLE, root_cause(&error)),
        Err(_elapsed) => (
            UNREACHABLE,
            format!("no answer within {}s", PROBE_TIMEOUT.as_secs()),
        ),
    }
}

/// `host:port` for a declared endpoint, in whichever form it is written.
///
/// The service-directory contract requires an origin URL
/// (`http://127.0.0.1:8895`), but a `tcp` endpoint carries no obligation to be
/// spelled with a scheme. An address this cannot resolve is `None`, never a
/// guess: filling in a default port would probe a process the declaration
/// never named and report the answer against a service that has nothing to do
/// with it.
fn socket_address(endpoint: &str) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(endpoint) {
        if let (Some(host), Some(port)) = (parsed.host(), parsed.port_or_known_default()) {
            return Some(format!("{host}:{port}"));
        }
    }
    let (host, port) = endpoint.rsplit_once(':')?;
    if host.is_empty() || port.parse::<u16>().is_err() {
        return None;
    }
    Some(endpoint.to_string())
}

/// The operating system's own words, not the wrapper's.
///
/// reqwest reports a dead endpoint as "error sending request for url (...)",
/// which names the URL the caller already knows and hides the one fact worth
/// having: refused, timed out, no route, DNS. Walking to the innermost cause
/// is the difference between a report an operator can act on and one more line
/// that looks like an application bug -- which is exactly how `fetch failed`
/// went unread for twelve days.
fn root_cause(error: &(dyn std::error::Error + 'static)) -> String {
    let mut deepest = error;
    while let Some(source) = deepest.source() {
        deepest = source;
    }
    let message = deepest.to_string();
    let trimmed = message.split(" for url").next().unwrap_or(&message).trim();
    if trimmed.is_empty() {
        return message;
    }
    trimmed.to_string()
}

/// Probe every declaration that names THIS host, from this host, by whatever
/// method each declaration carries.
async fn local_findings(registry: &Registry, me: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Some(directory) = registry.service_directory.as_ref() else {
        return findings;
    };
    for (name, service) in &directory.services {
        let descriptor = service.verification();
        if !probe_hosts(service, &descriptor).contains(me) {
            continue;
        }
        let endpoint = endpoint_for(service, me, &descriptor.kind);
        if let Some(detail) = unsupported(name, &descriptor) {
            findings.push(Finding {
                service: name.clone(),
                host: me.to_string(),
                endpoint: endpoint.unwrap_or_else(|| "-".to_string()),
                state: UNVERIFIED,
                detail,
            });
            continue;
        }
        match endpoint {
            None => findings.push(Finding {
                service: name.clone(),
                host: me.to_string(),
                endpoint: "-".to_string(),
                state: UNVERIFIED,
                detail: "no endpoint declared for this host".to_string(),
            }),
            Some(url) => {
                let (state, detail) = probe(&descriptor.kind, &url).await;
                findings.push(Finding {
                    service: name.clone(),
                    host: me.to_string(),
                    endpoint: url,
                    state,
                    detail,
                });
            }
        }
    }
    findings
}

/// Write down what was seen, where a later question can find it.
///
/// A probe that only prints has verified nothing five minutes from now: the
/// sweep runs, the table scrolls past, and the next component to ask "is this
/// declaration true" starts from zero and takes the declaration's own word for
/// it -- which is the position the fleet was in for twelve days. The record is
/// what lets an answer outlive the process that obtained it, and it is the
/// file [`crate::observations::freshness`] reads to decide whether an answer
/// is old enough to need asking again.
///
/// One record per finding, `unverified` ones included. "Nobody looked" is a
/// fact about the fleet worth keeping: it is the difference between a service
/// nobody has checked since Tuesday and one checked a minute ago.
///
/// A failed write is reported and never fatal. The rows on screen are true
/// regardless, and a full disk must not turn a working verifier into a command
/// that exits non-zero for a reason no service caused.
fn record_observations(findings: &[Finding]) {
    let observations: Vec<Observation> = findings
        .iter()
        .map(|finding| {
            Observation::now(
                service_fact(&finding.service, &finding.host),
                finding.host.clone(),
                finding.state,
                finding.detail.clone(),
            )
        })
        .collect();
    if let Err(error) = crate::observations::record(&observations) {
        eprintln!("could not record what was observed: {error}");
    }
}

/// `service verify --local`: what this host can actually reach, as JSON for the
/// sweep to collect, or a table when an operator runs it by hand on the box.
pub async fn verify_local(json_output: bool) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = load_registry_auto()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let me = registry
        .lookup_self(&hostname)
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .map(|target| target.name.clone())
        .ok_or_else(|| {
            CmdError::click(format!(
                "host {hostname} is not in {}; a machine the registry does not \
                 name cannot be a declared consumer of anything",
                crate::targets::registry_location()
            ))
        })?;
    let findings = local_findings(&registry, &me).await;
    record_observations(&findings);
    emit(&findings, json_output);
    fail_on_unreachable(&findings)
}

/// Run the probe on one remote host through the installed helper.
///
/// Not `ssh`, and not `host exec`: the exec allowlist carries fixed read-only
/// argv and cannot express "and then interpret this URL", while a helper that
/// took the URL as an argument would be a remote fetcher with the audit trail
/// removed. The helper takes no arguments at all -- it asks the same registry
/// this command is reading and probes the host's own share of it.
async fn remote_findings(host: &str, declared: &[(String, String)]) -> Vec<Finding> {
    let runner = crate::deploy::production_runner();
    let unverified = |detail: String| -> Vec<Finding> {
        declared
            .iter()
            .map(|(service, endpoint)| Finding {
                service: service.clone(),
                host: host.to_string(),
                endpoint: endpoint.clone(),
                state: UNVERIFIED,
                detail: detail.clone(),
            })
            .collect()
    };
    let output =
        match crate::deploy::host_channel::run_installed_helper(host, PROBE_HELPER, &runner).await {
            Ok(output) => output,
            Err(error) => return unverified(root_cause(&error)),
        };
    let parsed: Value = match serde_json::from_str(output.trim()) {
        Ok(parsed) => parsed,
        Err(error) => return unverified(format!("probe returned no usable JSON: {error}")),
    };
    let Some(rows) = parsed.as_array() else {
        return unverified("probe returned no usable JSON: not an array".to_string());
    };
    rows.iter()
        .map(|row| Finding {
            service: field(row, "service"),
            host: host.to_string(),
            endpoint: field(row, "endpoint"),
            state: match row.get("state").and_then(Value::as_str) {
                Some(OBSERVED) => OBSERVED,
                Some(UNREACHABLE) => UNREACHABLE,
                _ => UNVERIFIED,
            },
            detail: field(row, "detail"),
        })
        .collect()
}

fn field(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string()
}

/// `service verify`: sweep the whole directory from every vantage that holds a
/// declaration.
pub async fn verify(host: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let registry = load_registry_auto()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let Some(directory) = registry.service_directory.as_ref() else {
        return Err(CmdError::click(
            "the registry declares no service directory; there is nothing to verify",
        ));
    };
    let me = registry
        .lookup_self(&crate::providers::vast::system_hostname())
        .ok()
        .flatten()
        .map(|target| target.name.clone());

    // What each host is declared to hold, so a host that cannot be probed still
    // reports its declarations as unverified rather than vanishing from the
    // table. A missing row reads as "fine" to every operator alive.
    let mut per_host: std::collections::BTreeMap<String, Vec<(String, String)>> =
        std::collections::BTreeMap::new();
    for (name, service) in &directory.services {
        let descriptor = service.verification();
        for probed in probe_hosts(service, &descriptor) {
            if host.is_some_and(|only| only != probed) {
                continue;
            }
            let endpoint =
                endpoint_for(service, &probed, &descriptor.kind).unwrap_or_else(|| "-".to_string());
            per_host
                .entry(probed)
                .or_default()
                .push((name.clone(), endpoint));
        }
    }
    if per_host.is_empty() {
        return Err(CmdError::click(match host {
            Some(only) => format!("no service in the directory names host {only}"),
            None => "the service directory declares no hosts".to_string(),
        }));
    }

    let mut findings = Vec::new();
    for (target, declared) in &per_host {
        // Own host in-process: the local path is the same code the helper runs,
        // and requiring a helper on the machine already executing the command
        // would report `unverified` for the one vantage that is certain.
        if me.as_deref() == Some(target.as_str()) {
            findings.extend(local_findings(&registry, target).await);
            continue;
        }
        findings.extend(remote_findings(target, declared).await);
    }
    record_observations(&findings);
    emit(&findings, json_output);
    fail_on_unreachable(&findings)
}

fn emit(findings: &[Finding], json_output: bool) {
    if json_output {
        let rows: Vec<Value> = findings.iter().map(Finding::to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string())
        );
        return;
    }
    println!(
        "{:<22} {:<20} {:<34} {:<12} {}",
        "SERVICE", "HOST", "ENDPOINT", "STATE", "DETAIL"
    );
    for finding in findings {
        println!(
            "{:<22} {:<20} {:<34} {:<12} {}",
            finding.service, finding.host, finding.endpoint, finding.state, finding.detail
        );
    }
}

/// A false declaration is a failure, an unchecked one is not. Exiting non-zero
/// only on `unreachable` is what makes this usable as a gate without making an
/// uninstalled probe look like an outage.
fn fail_on_unreachable(findings: &[Finding]) -> Result<(), CmdError> {
    let broken = findings
        .iter()
        .filter(|finding| finding.state == UNREACHABLE)
        .count();
    if broken == 0 {
        return Ok(());
    }
    eprintln!(
        "{broken} declaration(s) point at an endpoint that answered nothing from the host \
         that is told to call it"
    );
    Err(CmdError::silent(1))
}
