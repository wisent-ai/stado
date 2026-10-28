//! `stado registry doctor` — the registry's declarations diffed against live
//! host state.
//!
//! The command is this one observation: one versioned read, one prefix
//! listing, and the checks that compare them. Each family of checks is its own
//! component — [`checks`] for the document and one target's beacon,
//! [`capability`] for the measured-capability join, [`declarations`] for
//! fields no consumer reads — and [`findings`] is the row they all produce.

pub(in crate::cli::registry) mod capability;
mod checks;
mod declarations;
mod findings;

use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{json, Value};

use crate::cli::registry::beacons::load::load_beacons;
use crate::cli::registry::doctor::capability::claims::requirement_findings;
use crate::cli::registry::doctor::checks::dedup::prune_symptoms;
use crate::cli::registry::doctor::checks::directory::{
    directory_candidate_ports, foreign_install_roots,
};
use crate::cli::registry::doctor::checks::target::target_findings;
use crate::cli::registry::doctor::checks::{
    declared_release_control, profiles_lacking_stores, resolver_refusal, schedules_pinned_nowhere,
};
use crate::cli::registry::doctor::declarations::{unread_configuration, unread_declarations};
use crate::cli::registry::doctor::findings::Finding;
use crate::cli::registry::echo_json;
use crate::cli::registry::write::document::fetch_versioned_document;
use crate::cli::{reporting::table, CmdError};
use crate::queue::{capacity, JobStorage};
use crate::targets;

/// `stado registry doctor [--json]` — diff registry declarations against
/// live host state: hosts with no heartbeat, stale beacons, missing
/// plists, unmanaged agents, and a build that refuses the document itself.
///
/// Exits non-zero on any divergence, naming each one, so it drops straight
/// into a cron or a CI gate. Liveness comes from the beacons and the
/// capacity broadcasts, never ssh: the whole command is one prefix listing
/// plus the bodies it finds.
#[allow(clippy::too_many_lines)]
pub async fn doctor(as_json: bool) -> Result<(), CmdError> {
    // Doctor needs the typed registry and raw extension blocks to describe one
    // observation. Derive both from one authoritative versioned read: using
    // `read_registry` here could select the last-known-good copy, and fetching
    // the raw document afterwards could then mix that copy with a different
    // authority generation.
    let (document, _) = fetch_versioned_document().await?;
    let registry = targets::load_registry_from_value(&document).map_err(|error| {
        CmdError::click(format!(
            "invalid registry document at {}: {error}",
            targets::registry_location()
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let store = JobStorage::for_primary_reads().await?;
    let beacons = load_beacons(&store).await?;
    let consumers = capacity::read_consumer_capacity(&store).await?;
    let now = Utc::now();

    let mut findings: Vec<Finding> = Vec::new();

    resolver_refusal(&document, &mut findings);
    let release_control = declared_release_control(&registry, &mut findings);
    directory_candidate_ports(&registry, release_control.as_ref(), &mut findings);
    foreign_install_roots(release_control.as_ref(), &mut findings);
    schedules_pinned_nowhere(&registry, &store, &mut findings).await;
    profiles_lacking_stores(&document, &mut findings);

    // The one host whose unit files this command may open. Everything else
    // here is read from the store, and the environment a unit actually
    // carries is in no object in it: the beacon publishes one `state` word
    // per unit and the registry's own service record has no environment
    // field at all, so this is the only host where the declaration can be
    // confronted with the file. A registry that names no target for this
    // machine leaves it `None`, and every unit is then reported unread
    // rather than empty.
    // An identity two targets claim is itself a registry finding: read as
    // "no target", it left every unit unread and the cause unsaid.
    let hostname = crate::providers::vast::system_hostname();
    let local_host = match registry.lookup_self(&hostname) {
        Ok(found) => found.map(|target| target.name.clone()),
        Err(error) => {
            findings.push(Finding::new(
                "ambiguous-host-identity",
                &hostname,
                format!("this machine's registry entry cannot be told, so its unit files are not read: {error}"),
            ));
            None
        }
    };

    // The raw document from the same versioned read that produced `registry`
    // above. `service_directory`, `service_resolver`, and release/build checks
    // need raw extension blocks, and a second fetch could answer with a
    // different generation. Doctor is an observation of the canonical
    // authority, so unlike general host-resolution commands it does not fall
    // back to the on-disk last-known-good copy. A refused primary read remains
    // a refusal instead of becoming findings about a stale document.

    let claimed = target_findings(
        &registry,
        &document,
        &beacons,
        release_control.as_ref(),
        local_host.as_deref(),
        now,
        &mut findings,
    )
    .await;

    for (slug, beacon) in &beacons {
        if !claimed.contains(slug) {
            findings.push(Finding::new(
                "unmanaged-host",
                slug,
                format!(
                    "{} is publishing beacons but no registry target claims that identity",
                    beacon.path
                ),
            ));
        }
    }

    for (consumer_id, payload) in &consumers {
        // Only local agents map one-to-one onto a registry box; a "gcp" or
        // "vast" broadcast comes from an ephemeral VM the registry
        // deliberately does not enumerate.
        if !payload
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| crate::capabilities::ProviderId::Local.matches(kind))
        {
            continue;
        }
        // consumer_id is "<kind>-<hostname>"
        // (`queue::capacity::publish_capacity`); `scheduler::makespan`
        // splits it exactly this way.
        let host = match consumer_id.split_once('-') {
            Some((_kind, host)) => host,
            // An id without the separator was published by a build that
            // did not prefix the kind; the whole id is then the host name.
            None => consumer_id.as_str(),
        };
        let declared = registry
            .lookup_self(host)
            .map_err(CmdError::from)?
            .is_some();
        if !declared {
            findings.push(Finding::new(
                "unmanaged-agent",
                consumer_id,
                format!(
                    "broadcasting live capacity as host {host}, which no registry target declares"
                ),
            ));
        }
    }

    // Three declaration checks the beacons cannot answer. They compare the
    // document with itself and with the last measurement rather than with a
    // heartbeat, so they run for every target, including the dispatcher pools
    // the liveness checks above skip.
    let entries: BTreeMap<&str, &Value> = match document.get("targets").and_then(Value::as_array) {
        Some(targets) => targets
            .iter()
            .filter_map(|entry| {
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| (name, entry))
            })
            .collect(),
        // A document without a targets array declares no target, and the
        // parsed registry below is then empty too.
        None => BTreeMap::new(),
    };
    for loop_back in crate::service_resolution::self_referencing_endpoints(&document)
        .map_err(CmdError::declaration)?
    {
        findings.push(Finding::new(
            "self-referencing-endpoint",
            &loop_back.target,
            format!(
                "service_directory.services.{}.{}.{} is {}, which is {} on that same target: the \
                 adapter would proxy to itself",
                loop_back.service,
                loop_back.map,
                loop_back.target,
                loop_back.address,
                loop_back.adapter
            ),
        ));
    }

    let (requirement_claims, declared_trajectories, capability_measurements) =
        requirement_findings(&store, &registry, now, &mut findings).await;

    for target in &registry.targets {
        // The registry's typed targets were parsed from this document, so
        // every one of them has an entry here; one without is a document
        // that changed under the doctor, and is named rather than read as
        // an empty entry.
        let Some(entry) = entries.get(target.name.as_str()).copied() else {
            findings.push(Finding::new(
                "registry-document-drift",
                &target.name,
                "the parsed registry names this target and the document read beside it carries \
                 no entry of that name"
                    .to_string(),
            ));
            continue;
        };
        findings.extend(unread_declarations(target, entry));
    }
    findings.extend(unread_configuration());

    prune_symptoms(&mut findings);

    let location = targets::registry_location();
    if as_json {
        echo_json(&json!({
            "registry": location,
            "ok": findings.is_empty(),
            "checked": {
                "targets": registry.targets.len(),
                "beacons": beacons.len(),
                "capacity_consumers": consumers.len(),
                "requirement_claims": requirement_claims,
                "declared_trajectories": declared_trajectories,
                "capability_measurements": capability_measurements,
            },
            "findings": findings.iter().map(Finding::to_json).collect::<Vec<Value>>(),
        }));
    } else if findings.is_empty() {
        println!(
            "registry {location} agrees with live host state ({} targets, {} beacons, \
             {} live consumers)",
            registry.targets.len(),
            beacons.len(),
            consumers.len()
        );
    } else {
        let rows: Vec<Vec<String>> = findings
            .iter()
            .map(|finding| {
                vec![
                    finding.kind.to_string(),
                    finding.subject.clone(),
                    finding.detail.clone(),
                ]
            })
            .collect();
        table::print(&["FINDING", "SUBJECT", "DETAIL"], &rows);
    }
    if findings.is_empty() {
        return Ok(());
    }
    // A negative verdict, not a fault: the command ran, read everything it
    // reads, and answered. Dressed as a `CmdError::click` it was classified
    // by its own wording, so an operator was told the command failed and we
    // could not attribute it to anything but their request or credentials
    // [unknown] — four false claims about a check that worked. The same
    // silent exit `release status`, `release version show`, `resolver status` and
    // `web route` verdicts use carries the one thing a gate owes its caller:
    // a non-zero code, and the count beside the two things compared.
    eprintln!(
        "{} divergence(s) between {location} and live host state; each is named above",
        findings.len()
    );
    Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE))
}
