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
//!   unverified   the probe could not run: host down, channel refused, the
//!                remote's own stado too old to answer. Kept apart from
//!                `unreachable` deliberately --
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
//! One ambiguity used to decide what this command called a failure, and it was
//! settled in the model rather than here. `Service::endpoints` is keyed by
//! host, and two readings survived the type: "the address this host uses to
//! reach the service", which is what `service directory publish` writes into
//! each host's `~/.stado/forwards/<service>.local`, and "where this host would
//! serve it if the service moved here", which is what the field's own comment
//! described. This command followed `publish`, because that is the code
//! consumers actually run -- and so reported `brama` unreachable on a laptop
//! that merely stands by for it, silenced at the time by a `from: active-host`
//! descriptor on that one entry.
//!
//! A descriptor on one entry was a patch, not a fix: the next standby address
//! added would have produced the same false report. The two meanings now have
//! two fields. `endpoints` is the address a host calls and nothing else;
//! [`crate::targets::Service::standby`] is the address a host would serve on
//! after a move, read through `address_for` and `standby_for` so no caller
//! has to guess which map answers its question. The model was the right place
//! because a command cannot resolve an ambiguity in the data it reads -- it
//! can only pick a reading and then be confidently wrong for everyone who
//! picked the other one, in a report that looks definite either way.
//!
//! Probing therefore uses `endpoints` alone. Standby addresses are listed as
//! their own `unverified` rows: visible, because an address nobody prints is
//! an address nobody maintains until the move that needs it, and never
//! failures, because nothing is supposed to answer on them yet.

// The three state words are imported, never respelled here. This command
// writes them into the observation record and other commands read them back
// out of it, so a private copy that drifted by one letter would file rows
// nothing matches -- a fact with no reader, which is the defect this change
// exists to remove.

mod checks;
mod finding;
mod probe;
mod verdicts;

use crate::cli::CmdError;
use crate::targets::load_registry_auto;

pub(crate) use crate::cli::service_verify::finding::Finding;

use crate::cli::service_verify::checks::local::local_findings;
use crate::cli::service_verify::checks::standby::standby_findings;
use crate::cli::service_verify::checks::{endpoint_for, probe_hosts};
use crate::cli::service_verify::finding::emit;
use crate::cli::service_verify::probe::remote::remote_findings;
use crate::cli::service_verify::verdicts::fail_on_unreachable;
use crate::cli::service_verify::verdicts::ownership::judge_ownership;
use crate::cli::service_verify::verdicts::record::record_observations;

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
    let mut findings = local_findings(&registry, &me).await;
    // The standby addresses this machine holds, printed beside what it can
    // actually reach. An operator on the box asking "what am I party to" is
    // owed the address it would serve on as well as the ones it calls; the
    // sweep reads them out of the directory itself and drops this copy.
    if let Some(directory) = registry.service_directory.as_ref() {
        findings.extend(standby_findings(directory, Some(me.as_str())));
    }
    record_observations(&findings);
    emit(&findings, json_output);
    fail_on_unreachable(&findings)
}

/// Collect and persist one reachability sweep without rendering it. The
/// coordinator uses this before service reconciliation so every mutation is
/// based on a fresh external observation, not yesterday's local cache.
pub(crate) async fn sweep(host: Option<&str>) -> Result<Vec<Finding>, CmdError> {
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
    let standby = standby_findings(directory, host);
    if per_host.is_empty() && standby.is_empty() {
        return Err(CmdError::click(match host {
            Some(only) => format!("no service in the directory names host {only}"),
            None => "the service directory declares no hosts".to_string(),
        }));
    }

    let mut findings = Vec::new();
    for (target, declared) in &per_host {
        if me.as_deref() == Some(target.as_str()) {
            findings.extend(local_findings(&registry, target).await);
        } else {
            findings.extend(remote_findings(target, declared).await);
        }
    }
    findings.extend(standby);
    judge_ownership(&registry, &mut findings).await;
    record_observations(&findings);
    Ok(findings)
}

/// `service verify`: sweep the whole directory from every declared vantage.
pub async fn verify(host: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let findings = sweep(host).await?;
    emit(&findings, json_output);
    fail_on_unreachable(&findings)
}
