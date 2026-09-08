//! One registry target read against itself and against the beacon that
//! proves what its host is actually running.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::cli::registry::beacons::beacon::Beacon;
use crate::cli::registry::beacons::load::beacon_for_slugs;
use crate::cli::registry::beacons::{stale_after_seconds, ACTIVE_STATE};
use crate::cli::registry::doctor::findings::Finding;
use crate::cli::registry::human_age;
use crate::deploy::service;
use crate::monitor::host_health;
use crate::targets::{self, ComputeTarget, Registry};

/// One service a registry target declares it manages.
struct DeclaredUnit {
    /// Operator-facing service name.
    name: String,
    /// The identifier the beacon reports it under: the launchd label on
    /// macOS, the systemd unit on Linux.
    id: String,
}

/// Services declared on a target by `stado service adopt|deploy`
/// (`deploy/service.rs`), which writes a per-target `services` array. The
/// key is unknown to [`ComputeTarget`], so it round-trips through
/// [`ComputeTarget::extra`]; a target that declares none is checked for
/// liveness only, never for units.
fn declared_units(
    target: &ComputeTarget,
    release_control: Option<&crate::release_control::ReleaseControl>,
) -> Vec<DeclaredUnit> {
    let legacy_labels = service::legacy_launchd_labels(target, release_control);
    let Some(services) = target.extra.get("services").and_then(Value::as_array) else {
        return Vec::new();
    };
    services
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
            };
            let id = text("label")
                .or_else(|| text("unit"))
                .or_else(|| text("name"))?;
            if legacy_labels.contains(id) {
                return None;
            }
            Some(DeclaredUnit {
                name: text("name").unwrap_or(id).to_string(),
                id: id.to_string(),
            })
        })
        .collect()
}

/// Every divergence one registry target earns, and the beacon slugs it
/// claims, so a beacon no target claims can be reported afterwards.
#[allow(clippy::too_many_lines)]
pub(in crate::cli::registry::doctor) async fn target_findings(
    registry: &Registry,
    document: &Value,
    beacons: &BTreeMap<String, Beacon>,
    release_control: Option<&crate::release_control::ReleaseControl>,
    local_host: Option<&str>,
    now: DateTime<Utc>,
    findings: &mut Vec<Finding>,
) -> BTreeSet<String> {
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    for target in &registry.targets {
        let slugs = host_health::beacon_slugs(target, &target.name);
        claimed.extend(slugs.iter().cloned());
        // Only kind=local declares a machine that runs a beacon; "gcp" and
        // "vast" targets are dispatcher pools, not boxes.
        if !target.is_provider(crate::capabilities::ProviderId::Local) {
            continue;
        }
        // The document against itself, before any beacon is consulted: a
        // launchd domain the host cannot have is wrong whether or not the
        // host is answering, and it is the reason its beacon will never
        // report the unit.
        for misdeclared in service::misdeclared_domains(target) {
            let unit = misdeclared.unit.clone();
            findings.push(
                Finding::new("misdeclared-domain", &target.name, misdeclared.sentence())
                    .about(unit),
            );
        }
        // `managed_versions` belongs only to the compiled `host release`
        // catalog. Release-control products already carry their desired
        // version in that block, while arbitrary `service update` trees carry
        // an artifact identity instead of an invented semver contract.
        for undeclared in service::managed_units_without_declared_version(target, release_control) {
            findings.push(
                Finding::new(
                    "undeclared-service-version",
                    &target.name,
                    undeclared.sentence(),
                )
                .about(undeclared.unit),
            );
        }
        // The same shape again, and the reason this one needed the check
        // extended rather than a second one built beside it: both loops
        // above resolve `policy.targets.get(host)` before they compare
        // anything, so a host no product target names is their skip
        // condition instead of their finding — and that host is exactly the
        // one a product's declared environment cannot reach.
        for unreachable in
            service::unreachable_product_environments(target, release_control, local_host)
        {
            findings.push(
                Finding::new(unreachable.kind(), &target.name, unreachable.sentence())
                    .about(unreachable.unit.clone()),
            );
        }
        // The last of the pre-beacon checks, and the one the beacon could
        // never have answered: which FILE a unit's live process is executing.
        // The beacon publishes one `state` word per unit and a unit running
        // an obsolete build is `active` by every measure it takes, so this is
        // read off the process table and the kernel rather than out of the
        // store — and therefore only on the host this command runs on.
        //
        // `com.wisent.compute.disk-cleanup.disk-cleanup` spent thirteen days
        // journalling `policy:ValueError` on 8,348 passes from a `--watch`
        // process that had been alive since 27 August, executing an image the
        // file underneath it no longer held. Nothing revisited it, because
        // `self_update::recycle_replaced_units` cycles a unit only inside the
        // invocation that replaced its bytes; an unrelated restart is what
        // ended it.
        // The revisit ledger is one host-wide file answering one question, so
        // it is opened once for the whole pass rather than once per finding.
        // `None` unless this is the local host and some product authorised a
        // unit on it, which is no host today.
        let revisit = crate::release_unit_image::annotations(document, &target.name, local_host);
        for image in
            service::units_running_replaced_images(target, local_host, now.timestamp()).await
        {
            // The row that told an operator to restart the unit by hand is
            // the row that has to say the release agent already tried and
            // what came back. Same kind, same sentence, one clause longer: a
            // repair that happens silently is the same defect as a failure
            // that happens silently, and a new severity word for it would be
            // a third vocabulary for one condition. Only for units an enabled
            // policy explicitly owns.
            let mut sentence = image.sentence();
            if let Some(clause) = revisit.as_ref().and_then(|revisit| revisit.clause(&image)) {
                sentence.push_str(&clause);
            }
            let mut finding = Finding::new(image.kind(), &target.name, sentence);
            // The row that reports a whole host unread names no unit, and
            // attaching an empty label to it would let the `missing-plist`
            // de-duplication downstream match on the empty string.
            if !image.unit.is_empty() {
                finding = finding.about(image.unit.clone());
            }
            findings.push(finding);
        }
        // The condition that opened both silent windows, and the one no check
        // above can see: the build is refusing the document. Nothing had been
        // replaced when either window opened — the installed file and the
        // running image agreed, which is why `units_running_replaced_images`
        // fires nothing — and the REGISTRY was what moved. `resolver status`
        // learned to publish this for the resolver's own process (#345); this
        // is the same fault measured for the build an operator is holding,
        // on the surface that carries every other kind of drift.
        //
        // Local-only, and every other machine gets its unmeasured row, for
        // the reason `observe_unit_images` states about pids: a build's
        // verdict is knowable only by running that build.
        for skew in targets::builds_refusing_registry(&target.name, document, local_host) {
            findings.push(Finding::new(skew.kind(), &target.name, skew.sentence()));
        }
        let Some(beacon) = beacon_for_slugs(&slugs, beacons) else {
            findings.push(Finding::new(
                "no-heartbeat",
                &target.name,
                format!(
                    "declared kind=local but no beacon exists; checked {}/{{{}}}.json",
                    host_health::HEALTH_PREFIX,
                    slugs.join(",")
                ),
            ));
            continue;
        };
        match beacon.observed_at() {
            Some(observed) => {
                let age = now - observed;
                if age.num_seconds() > stale_after_seconds() {
                    findings.push(Finding::new(
                        "stale-beacon",
                        &target.name,
                        format!(
                            "{} last updated {} ago ({}), past the {}s liveness window",
                            beacon.path,
                            human_age(age),
                            observed.to_rfc3339(),
                            stale_after_seconds()
                        ),
                    ));
                }
            }
            None => findings.push(Finding::new(
                "stale-beacon",
                &target.name,
                format!(
                    "{} carries neither an object timestamp nor reported_at",
                    beacon.path
                ),
            )),
        }
        for declared in declared_units(target, release_control) {
            match beacon.unit_state(&declared.id) {
                None => findings.push(
                    Finding::new(
                        "missing-plist",
                        &target.name,
                        format!(
                            "registry declares service {} ({}) but {} reports no such unit",
                            declared.name, declared.id, beacon.path
                        ),
                    )
                    .about(declared.id.as_str()),
                ),
                Some(state)
                    if state != ACTIVE_STATE && !beacon.scheduled_unit_is_healthy(&declared.id) =>
                {
                    findings.push(
                        Finding::new(
                            "unit-not-active",
                            &target.name,
                            format!(
                                "registry declares service {} ({}) but {} reports state={state}",
                                declared.name, declared.id, beacon.path
                            ),
                        )
                        .about(declared.id.as_str()),
                    )
                }
                Some(_) => {}
            }
        }
    }
    claimed
}
