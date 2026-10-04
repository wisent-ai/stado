//! Verify service declarations from the hosts that consume them.
//!
//! Schema validation cannot establish reachability. This command drives each
//! entry's verification descriptor: probe kind, vantage and required answer.
//! A serving-host probe alone does not prove that another consumer can reach
//! the endpoint it was given.
//!
//! Reachability outcomes distinguish an observed answer, an unreachable
//! endpoint, and a probe that could not run. Unsupported descriptors remain
//! `unverified` with their cause; registry validation also reports unsupported
//! kinds or vantages. An omitted descriptor uses the model's default.
//!
//! `Service::endpoints` names consumer addresses. `Service::standby` names
//! addresses a host would serve after placement changes; `address_for` and
//! `standby_for` keep those meanings separate.
//!
//! Consumer probes use `endpoints`. Standby addresses remain visible as
//! unverified declarations and are dialled from their own hosts. Silence at a
//! standby is expected; an answer is `standby_serving`, an additional serving
//! copy and a failed sweep. Listener ownership is judged separately from
//! whether the socket answered. Outcomes use the shared observation vocabulary.

mod checks;
mod finding;
mod probe;
mod verdicts;

use crate::cli::CmdError;
use crate::targets::load_registry_auto;

pub(crate) use crate::cli::service_verify::checks::recheck::{
    standby_still_serving, StandbyRecheck,
};
pub(crate) use crate::cli::service_verify::finding::Finding;

use crate::cli::service_verify::checks::local::local_findings;
use crate::cli::service_verify::checks::standby::{
    merge_serving, serving_standbys, standby_findings,
};
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
    let registry = load_registry_auto().await.map_err(CmdError::from)?;
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
    // sweep reads them out of the directory itself and keeps only the rows
    // this host found serving, which no directory read can produce.
    if let Some(directory) = registry.service_directory.as_ref() {
        let mut standby = standby_findings(directory, Some(me.as_str()));
        merge_serving(
            &mut standby,
            serving_standbys(&registry, directory, &me).await,
        );
        findings.extend(standby);
    }
    record_observations(&findings);
    emit(&findings, json_output);
    fail_on_unreachable(&findings)
}

/// Collect and persist one reachability sweep without rendering it. The
/// coordinator uses this before service reconciliation so every mutation is
/// based on a fresh external observation, not yesterday's local cache.
pub(crate) async fn sweep(host: Option<&str>) -> Result<Vec<Finding>, CmdError> {
    let registry = load_registry_auto().await.map_err(CmdError::from)?;
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
    let mut standby = standby_findings(directory, host);
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
    // A standby answers only from its own host: this one directly, the others
    // through their own `--local`, which reports a serving standby as a
    // probed row. A host holding nothing but a standby address is visited too,
    // because that is exactly the host a serving copy hides on.
    let mut serving = Vec::new();
    if let Some(local) = me
        .as_deref()
        .filter(|name| host.is_none_or(|only| only == *name))
    {
        serving.extend(serving_standbys(&registry, directory, local).await);
    }
    let standby_only: std::collections::BTreeSet<String> = standby
        .iter()
        .map(|row| row.host.clone())
        .filter(|name| !per_host.contains_key(name) && me.as_deref() != Some(name.as_str()))
        .collect();
    for target in &standby_only {
        serving.extend(remote_findings(target, &[]).await);
    }
    let (serving_rows, other): (Vec<Finding>, Vec<Finding>) = findings
        .into_iter()
        .partition(|finding| finding.state == crate::observations::STANDBY_SERVING);
    findings = other;
    serving.extend(serving_rows);
    // Keep the serving rows, and this host's standby rows whose answer came
    // from an owner other than the standby unit (unprobed, with that detail).
    serving
        .retain(|finding| finding.state == crate::observations::STANDBY_SERVING || !finding.probed);
    merge_serving(&mut standby, serving);
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
