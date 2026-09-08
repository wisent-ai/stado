//! The two entry points and everything they share: one deadline per host, the
//! checks answered from the registry document alone, and the questions one
//! host is asked.

use std::time::Duration;

use super::loaded::environment::unit_environment;
use super::loaded::orphans::loaded_without_unit_file;
use super::loaded::restarts::restart_loops;
use super::loaded::shadows::path_binary;
use super::loaded::{duplicate_domains, process_identity};
use super::naming::{declared_doubled_prefix, doubled_prefix, doubled_suffix, profile_unit_names};
use super::resources::artefacts::service_artefacts;
use super::resources::disk::disk_headroom;
use super::resources::listener_count;
use super::resources::stores::replica_addressing;
use super::{
    Finding, Measurement, Sweep, ORPHAN_CHECK, PREFIX_CHECK, RESTART_CHECK, SHADOW_CHECK,
    SUFFIX_CHECK, UNIT_ENV_CHECK,
};
use crate::deploy::{host_channel, service, Runner};
use crate::targets::{ComputeTarget, Registry};

/// Per-host wall clock. A host that has gone quiet must cost one line, not the tick. Three remote
/// tick it was swept from.
const HOST_TIMEOUT: Duration = Duration::from_secs(240);

/// Sweep the whole canonical registry.
///
/// Never returns an error: an unreachable host is a recorded fact, because the
/// useful output is the whole list and one dead box must not suppress it.
pub async fn sweep(runner: &Runner) -> Sweep {
    let mut result = Sweep::default();
    let registry = match host_channel::canonical_registry().await {
        Ok(registry) => registry,
        Err(error) => {
            result
                .unreachable
                .push(("<registry>".to_string(), error.to_string()));
            return result;
        }
    };
    // The store-addressing check is answered from configuration alone, so it
    // runs even when every host is unreachable.
    replica_addressing(&mut result);
    // Declared-name checks are answered from the registry alone, so they run
    // for every target, including hosts that answer nothing. Deliberately not
    // inside `host_findings`, which returns before any of its checks when the
    // host's loaded-unit read fails — and a host carrying a doubled unit name
    // is exactly the host whose units cannot be read, so placing it there
    // measured zero subjects on the two hosts that had the defect. A check that
    // only runs where the defect is absent is the shape this module refuses.
    declared_names(&registry, &mut result);
    for target in registry
        .targets
        .iter()
        .filter(|target| crate::capabilities::ProviderId::Local.matches(&target.kind))
    {
        sweep_host(&registry, target, runner, &mut result).await;
    }
    result
}

/// Everything one host is asked, under one deadline.
async fn sweep_host(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
    result: &mut Sweep,
) {
    match tokio::time::timeout(HOST_TIMEOUT, host_findings(registry, target, runner)).await {
        Ok(Ok((mut findings, mut notes, mut measurements))) => {
            result.measured += 1;
            for finding in findings.drain(..) {
                result.record(finding);
            }
            result.notes.append(&mut notes);
            result.measurements.append(&mut measurements);
        }
        Ok(Err(error)) => result.unreachable.push((target.name.clone(), error)),
        Err(_) => result.unreachable.push((
            target.name.clone(),
            format!("did not answer within {}s", HOST_TIMEOUT.as_secs()),
        )),
    }
}

/// Every check answered from the registry document alone.
fn declared_names(registry: &Registry, result: &mut Sweep) {
    let mut findings = Vec::new();
    let mut suffixes = 0_usize;
    let mut labels = 0_usize;
    for target in &registry.targets {
        doubled_suffix(target, &mut findings, &mut suffixes);
        declared_doubled_prefix(target, &mut findings, &mut labels);
    }
    profile_unit_names(registry, &mut findings, &mut labels, &mut suffixes);
    result.measured += 1;
    for finding in findings.drain(..) {
        result.record(finding);
    }
    for (check, measured, population) in [
        (SUFFIX_CHECK, suffixes, "declared systemd unit(s)"),
        (PREFIX_CHECK, labels, "declared launchd label(s)"),
    ] {
        result.notes.push(format!(
            "measured {check} across the registry: {measured} {population}{}",
            if measured == 0 {
                " — ZERO, so this check proved nothing"
            } else {
                ""
            }
        ));
        result
            .measurements
            .push(Measurement::new(check, None, measured as u64));
    }
}

/// What one host answered: things to act on, and things measured that need no
/// action. Both are returned, because a check that stays silent when it found
/// nothing to report is indistinguishable from one that never ran.
async fn host_findings(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<(Vec<Finding>, Vec<String>, Vec<Measurement>), String> {
    let mut findings = Vec::new();
    let mut notes = Vec::new();
    let (loaded, posture) = service::loaded_units_with_posture(target, runner)
        .await
        .map_err(|error| error.to_string())?;
    duplicate_domains(target, &loaded, &mut findings);
    process_identity(target, &loaded, &mut findings, &mut notes);
    // The five checks added on 2026-09-02, each one a state that was found by
    // hand during the #286 hunt and that nothing would have reported again.
    let mut labels_measured = 0_usize;
    let mut runs_measured = 0_usize;
    let mut env_measured = 0_usize;
    let mut path_measured = 0_usize;
    doubled_prefix(target, &loaded, &mut findings, &mut labels_measured);
    let mut orphan_measured = 0_usize;
    loaded_without_unit_file(target, &loaded, &mut findings, &mut orphan_measured);
    restart_loops(target, &loaded, &mut findings, &mut runs_measured);
    unit_environment(target, &loaded, &mut findings, &mut env_measured);
    path_binary(
        target,
        posture.as_ref(),
        &mut findings,
        &mut notes,
        &mut path_measured,
    );
    // A check that cannot say what it looked at cannot be trusted when it is
    // quiet, which is the rule this module holds itself to. One note per
    // check, each naming the check's own id, because the single prose sentence
    // this replaced named none of them: "983 label(s) read for a doubled
    // prefix" cannot be matched to `label-carries-its-prefix-once` by anything
    // but a human who already knows the code, so "how many subjects did this
    // check interrogate" was unanswerable from `doctor --json` even though the
    // number was right there.
    let mut measurements = Vec::new();
    for (check, measured) in [
        (PREFIX_CHECK, labels_measured),
        (ORPHAN_CHECK, orphan_measured),
        (RESTART_CHECK, runs_measured),
        (UNIT_ENV_CHECK, env_measured),
        (SHADOW_CHECK, path_measured),
    ] {
        notes.push(format!(
            "measured {check} on {}: {measured} subject(s){}",
            target.name,
            if measured == 0 {
                " — ZERO, so this check proved nothing here"
            } else {
                ""
            }
        ));
        measurements.push(Measurement::new(
            check,
            Some(target.name.clone()),
            measured as u64,
        ));
    }
    // One inventory read, two questions: which processes hold which declared
    // ports, and whether the artefacts behind the service units are the ones
    // the fleet has installed.
    if let Some(reading) = listener_count(registry, target, runner, &mut findings).await {
        service_artefacts(target, &reading, &mut findings, &mut notes);
    }
    disk_headroom(target, runner, &mut findings, &mut notes).await;
    Ok((findings, notes, measurements))
}
