//! The checks `registry doctor` runs: the document read against itself here,
//! the service directory against release control ([`directory`]), one target
//! against its beacon ([`target`]), and the symptom rows one cause makes
//! redundant ([`dedup`]).

mod declared;
pub(super) mod dedup;
pub(super) mod directory;
pub(super) mod target;

use serde_json::Value;

use crate::cli::registry::doctor::findings::Finding;
use crate::targets::Registry;

/// The build's own verdict on the document every resolver on the fleet reads.
pub(super) fn resolver_refusal(document: &Value, findings: &mut Vec<Finding>) {
    // A document the inference contract refuses is not a cosmetic fault: every
    // resolver on the fleet validates the same way before it adopts a
    // generation, so it keeps serving the last copy it accepted and hands
    // consumers an address the fleet has since moved away from. A route
    // alias without a namespace publishes that state: the always-on host's
    // resolver freezes many generations back, every chat takes `connection
    // refused` from a candidate port nothing serves any more, and the only
    // trace is one line in that resolver's log.
    if let Err(error) = crate::inference::schema::validate(document) {
        findings.push(Finding::new(
            "resolver-refuses-registry",
            "registry",
            format!(
                "every resolver refuses this document and keeps serving the last \
                 generation it accepted, so consumers resolve to addresses this \
                 registry no longer declares: {error}"
            ),
        ));
    }
}

/// The release-control block every version and environment check below is
/// measured against.
pub(super) fn declared_release_control(
    registry: &Registry,
    findings: &mut Vec<Finding>,
) -> Option<crate::release_control::ReleaseControl> {
    // What the fleet declares DELIVERED, which is what a missing version
    // declaration is measured against. Read from the document's own
    // `release_control` block and never from a unit on the host: a product
    // stays a release target after its launchd plist is removed, and so does
    // the version gap. A block that will not parse is reported rather than
    // skipped — a check that quietly measures nothing is the defect it was
    // written to catch.
    match registry
        .extra
        .get(crate::release_control::RELEASE_CONTROL_KEY)
    {
        Some(value) => {
            match <crate::release_control::ReleaseControl as serde::Deserialize>::deserialize(value)
            {
                Ok(control) => Some(control),
                Err(error) => {
                    findings.push(Finding::new(
                        "unreadable-release-control",
                        "registry",
                        format!(
                            "registry.{} did not parse, so no delivered product was judged \
                             against its declared version: {error}",
                            crate::release_control::RELEASE_CONTROL_KEY
                        ),
                    ));
                    None
                }
            }
        }
        None => None,
    }
}

/// Every live schedule whose pin names no host the registry declares, by
/// name or by a `<kind>-<hostname>` identity (the rule the claim path uses):
/// its jobs wait for a claimer that never comes. A store that does not
/// answer is a finding too, never read as no schedules.
pub(super) async fn schedules_pinned_nowhere(
    registry: &Registry,
    store: &crate::queue::JobStorage,
    findings: &mut Vec<Finding>,
) {
    let schedules = match crate::schedules::list_schedules(store).await {
        Ok(schedules) => schedules,
        Err(error) => {
            findings.push(Finding::new(
                "unreadable-schedules",
                "schedules",
                format!(
                    "the schedule store did not answer, so no schedule's pin was judged: {error}"
                ),
            ));
            return;
        }
    };
    for schedule in schedules
        .into_iter()
        .filter(|schedule| !schedule.deleted && !schedule.pinned_host.is_empty())
    {
        let mut named = false;
        for target in &registry.targets {
            match crate::deploy::host_gates::resolves_to(registry, target, &schedule.pinned_host) {
                Ok(resolves) => named |= resolves || target.name == schedule.pinned_host,
                Err(error) => {
                    // One identity two targets claim answers the same for
                    // every target, so it is said once.
                    findings.push(Finding::new(
                        "ambiguous-schedule-pin",
                        &schedule.schedule_id,
                        format!(
                            "schedule {} is pinned to {}: {error}",
                            schedule.schedule_id, schedule.pinned_host
                        ),
                    ));
                    named = true;
                    break;
                }
            }
        }
        if !named {
            findings.push(Finding::new(
                "schedule-pinned-nowhere",
                &schedule.schedule_id,
                format!(
                    "schedule {} is pinned to {}, which no registry target names, so its jobs wait for \
                     a claimer that never comes; declare it again with stado schedule create --id {} \
                     --converge and --pinned-host <host>, or, when its host was emptied onto another, \
                     evacuate with --repin-schedules",
                    schedule.schedule_id, schedule.pinned_host, schedule.schedule_id
                ),
            ));
        }
    }
}

/// Every placement profile that lacks a store the catalog declares for a
/// product it runs: a move of it would leave that store on the fenced
/// source, so `placement move` refuses it until `profile sync` adds
/// what the catalog declares. Named here so the gap is seen before a move.
pub(super) fn profiles_lacking_stores(document: &Value, findings: &mut Vec<Finding>) {
    let profiles = match crate::placement::profiles(document) {
        Ok(profiles) => profiles,
        Err(error) => {
            findings.push(Finding::new(
                "unreadable-placement-profiles",
                "registry",
                format!("the registry's placement profiles did not parse, so none was judged against the catalog's stores: {error}"),
            ));
            return;
        }
    };
    for profile in &profiles {
        match crate::cli::placement::profile_lacking_stores(profile) {
            Ok(lacking) if lacking.is_empty() => {}
            Ok(lacking) => findings.push(Finding::new(
                "profile-lacks-catalog-stores",
                &profile.name,
                format!(
                    "placement profile {} does not carry {}, which the catalog declares for what it runs, so a move would leave it on the source; add it with stado placement profile sync {}",
                    profile.name,
                    crate::cli::placement::listed_stores(&lacking),
                    profile.name
                ),
            )),
            Err(error) => findings.push(Finding::new(
                "profile-stores-unjudged",
                &profile.name,
                format!("placement profile {}'s stores could not be judged against the catalog: {error}", profile.name),
            )),
        }
    }
}
