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

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::targets::{load_registry_auto, Registry, Service};

/// Helper that runs this same command in `--local` mode on a remote host.
/// Installed with
/// `stado host install-helper <target> stado-rs/scripts/probe-service-endpoints.sh
/// probe-service-endpoints`.
const PROBE_HELPER: &str = "probe-service-endpoints";

/// A probe must not hang a fleet sweep behind one dead forward. Long enough for
/// a loopback service under load, short enough that a closed laptop answers
/// promptly with the truth.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

const OBSERVED: &str = "observed";
const UNREACHABLE: &str = "unreachable";
const UNVERIFIED: &str = "unverified";

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

/// Which hosts hold an address for this service?
///
/// The `endpoints` map, keyed by registry target, is what a consumer actually
/// reads: `service directory publish` writes `endpoints[<this host>]` into
/// `~/.stado/forwards/<service>.local` and skips a service that gives this host
/// no entry. So a host carrying an endpoint has been handed an address and can
/// be held to it, whether or not it is the one serving.
///
/// Deliberately NOT `consumers`. That map is keyed by consumer identity -- the
/// name of the software calling in, like `weles` -- and not by host, so reading
/// it as a host set produces confident answers about machines that do not
/// exist. The consumer-to-host mapping lives in placement, not here, and
/// guessing at it would put this command in the same class of defect it was
/// written to catch.
fn interested_hosts(service: &Service) -> BTreeSet<String> {
    let mut hosts: BTreeSet<String> = service.endpoints.keys().cloned().collect();
    hosts.insert(service.active_host.clone());
    hosts
}

/// The URL a given host is told to use, plus the health path when the endpoint
/// carries one. An endpoint with no entry for a host that is supposed to call
/// it is itself a finding: the consumer has been authorized and given no address.
fn endpoint_for(service: &Service, host: &str) -> Option<String> {
    let endpoint = service.endpoints.get(host)?;
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

/// Ask the endpoint whether anything is there.
///
/// Any HTTP response counts as observed, including 401, 404 and 503. This
/// verifies that the declaration points at something serving, not that the
/// service is healthy: a 503 from a real server is a different and much smaller
/// problem than a connection that goes nowhere, and conflating them is what let
/// `fetch failed` sit in a log for twelve days looking like an application bug.
async fn probe(url: &str) -> (&'static str, String) {
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

/// Probe every declaration that names THIS host, from this host.
async fn local_findings(registry: &Registry, me: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Some(directory) = registry.service_directory.as_ref() else {
        return findings;
    };
    for (name, service) in &directory.services {
        if !interested_hosts(service).contains(me) {
            continue;
        }
        match endpoint_for(service, me) {
            None => findings.push(Finding {
                service: name.clone(),
                host: me.to_string(),
                endpoint: "-".to_string(),
                state: UNVERIFIED,
                detail: "no endpoint declared for this host".to_string(),
            }),
            Some(url) => {
                let (state, detail) = probe(&url).await;
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
        for interested in interested_hosts(service) {
            if host.is_some_and(|only| only != interested) {
                continue;
            }
            let endpoint = endpoint_for(service, &interested).unwrap_or_else(|| "-".to_string());
            per_host
                .entry(interested)
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
