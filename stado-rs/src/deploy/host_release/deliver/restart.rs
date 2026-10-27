use std::collections::BTreeMap;
use std::num::NonZeroU64;

use serde_json::Value;

use super::super::ReleasePlan;
use super::step_entry;
use crate::deploy::{host_channel, service, service_label_print, Runner};
use crate::targets::{ComputeTarget, Registry};

/// The host a unit's service is placed on, when that is not `host`: a
/// directory service whose managed unit is `unit`, or a placement profile
/// that declares `unit` on `host` while its services are active elsewhere.
/// `None` when the unit serves nothing the registry places on another host.
fn placed_elsewhere(registry: &Registry, host: &str, unit: &str) -> Option<String> {
    let services = registry
        .service_directory
        .as_ref()
        .map(|directory| &directory.services)?;
    let active_elsewhere = |name: &String| {
        services
            .get(name)
            .map(|service| service.active_host.clone())
            .filter(|active| active != host)
    };
    for (name, service) in services {
        if service.managed_service.as_deref() == Some(unit) {
            if let Some(active) = active_elsewhere(name) {
                return Some(active);
            }
        }
    }
    registry
        .placement_profiles
        .iter()
        .filter(|profile| {
            profile
                .hosts
                .get(host)
                .is_some_and(|declared| declared.units.values().any(|placed| placed.unit == unit))
        })
        .find_map(|profile| {
            profile
                .services
                .iter()
                .find_map(|name| active_elsewhere(name))
        })
}

/// The exit status a restart step that failed without one of its own reports:
/// the platform's general failure status.
const STEP_FAILED: i32 = nix::libc::EXIT_FAILURE;

/// A field of a process identity as the failure line prints it: its value, or
/// the word `absent` when the init system did not report it, so a missing
/// field is said to be missing rather than printed as an empty or zero value
/// it never had.
fn field(value: Option<impl ToString>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "absent".to_string(),
    }
}

/// Restart every declared unit that was loaded before activation and join
/// each new init-system pid to the image digest activation reported, in the
/// order the units were declared. A unit the init system answered for and
/// did not hold runs no earlier release, so there is nothing to recycle: it
/// is left unloaded, and starts the activated program whenever its own
/// lifecycle starts it. Loading it here would start a second copy of a
/// service placed on another host, which is exactly the state a standby host
/// keeps its staged units in.
///
/// The failures are returned rather than reported here so the caller closes the
/// report exactly where it closed it before, at the phase that owns it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn restart_units(
    target: &ComputeTarget,
    plan: &ReleasePlan,
    units: &[service::ManagedService],
    already_proven_units: &BTreeMap<String, Value>,
    prior_unit_state: &BTreeMap<String, service_label_print::LabelState>,
    active_sha256: &str,
    steps: &mut Vec<Value>,
    runner: &Runner,
) -> (Vec<Value>, Vec<(i32, String)>) {
    let mut unit_processes = Vec::new();
    let mut failures = Vec::new();
    if units.is_empty() {
        steps.push(step_entry(
            "restart",
            "no_declared_units",
            Some(format!(
                "the registry declares no units running {} on {}",
                plan.product.name, target.name
            )),
        ));
        return (unit_processes, failures);
    }
    // Read once, and only when a unit the init system did not hold has to
    // be placed; the read answers the cached last good registry when the
    // fleet store does not answer.
    let mut registry: Option<Result<Registry, String>> = None;
    for declared in units {
        let unit_id = declared.unit_id().to_string();
        if let Some(evidence) = already_proven_units.get(&unit_id) {
            steps.push(step_entry(
                "restart",
                "already_mapped",
                Some(format!(
                    "{unit_id} already maps the activated immutable image"
                )),
            ));
            unit_processes.push(evidence.clone());
            continue;
        }
        let previous = prior_unit_state.get(&unit_id);
        // A unit the init system did not hold is started only where its
        // service is placed: on a host that stands by for a service placed
        // elsewhere, starting it would run a second copy. A placed unit that
        // crashed, was booted out or was never installed is started.
        if previous.is_some_and(service_label_print::LabelState::absent) {
            if registry.is_none() {
                registry = Some(
                    host_channel::canonical_registry()
                        .await
                        .map_err(|error| error.to_string()),
                );
            }
            let placement = match registry.as_ref() {
                Some(Ok(registry)) => placed_elsewhere(registry, &target.name, &unit_id)
                    .map(|active| ("not_loaded", format!("{unit_id} was not loaded before activation and its service is placed on {active}, so it stays unloaded here; it starts when a placement move brings the service to {}", target.name))),
                Some(Err(error)) => Some(("placement_unread", format!("{unit_id} was not loaded before activation and the registry that says where its service is placed could not be read ({error}), so it is left unloaded rather than started on a guess"))),
                None => None,
            };
            if let Some((status, detail)) = placement {
                steps.push(step_entry("restart", status, Some(detail)));
                continue;
            }
        }
        let arguments = std::iter::once(declared.program.as_str())
            .chain(declared.args.iter().map(String::as_str))
            .collect::<Vec<_>>();
        if crate::self_update::defers_to_release_handshake(&arguments) {
            if let Some(evidence) = previous {
                unit_processes.push(evidence.to_json());
            }
            steps.push(step_entry(
                "restart",
                "deferred_to_release_handshake",
                Some(format!(
                    "{unit_id} owns a queue worker and will recycle itself after its active jobs"
                )),
            ));
            continue;
        }
        let failure = match service::restart_service(target, declared, runner).await {
            Ok(restarted) if restarted.succeeded("restarted") => {
                match service_label_print::print_label(
                    target,
                    declared.unit_id(),
                    service::BootoutScope::Any,
                    runner,
                )
                .await
                {
                    Ok(current) => {
                        let previous_pid = previous.and_then(|state| state.pid.as_deref());
                        let previous_start =
                            previous.and_then(|state| state.process_started_at.as_deref());
                        let current_pid = current.pid.as_deref();
                        let current_start = current.process_started_at.as_deref();
                        let fresh = current_pid.is_some()
                            && current_start.is_some()
                            && (current_pid != previous_pid || current_start != previous_start);
                        let exact_image = current.process_executable.is_some()
                            && current.process_device.is_some()
                            && current.process_inode.and_then(NonZeroU64::new).is_some()
                            && (plan.product.install.is_tree()
                                || current.process_sha256.as_deref() == Some(active_sha256));
                        unit_processes.push(current.to_json());
                        if fresh && exact_image {
                            let detail = format!(
                                "{unit_id} fresh pid {} image {} sha256 {}",
                                field(current.pid.as_deref()),
                                field(current.process_executable.as_deref()),
                                field(current.process_sha256.as_deref())
                            );
                            steps.push(step_entry("restart", "ok", Some(detail)));
                            None
                        } else {
                            let mut detail = format!(
                                "{unit_id}: restart did not prove a fresh pid on the activated \
                                 immutable image; prior pid={} start={} current pid={} start={} \
                                 executable={} device={} inode={} sha256={} \
                                 identity_unavailable={}",
                                field(previous_pid),
                                field(previous_start),
                                field(current_pid),
                                field(current_start),
                                field(current.process_executable.as_deref()),
                                field(current.process_device),
                                field(current.process_inode),
                                field(current.process_sha256.as_deref()),
                                field(current.process_identity_unavailable.as_deref())
                            );
                            // A unit its supervisor keeps restarting is the
                            // release that cannot serve: say so with the
                            // supervisor's own exit code and run count.
                            if let Some(failing) = current.failing() {
                                detail = format!("{detail}; {failing}");
                            }
                            Some((STEP_FAILED, detail))
                        }
                    }
                    Err(error) => Some((
                        STEP_FAILED,
                        format!("{unit_id}: process identity read failed: {error}"),
                    )),
                }
            }
            Ok(restarted) => Some((
                restarted.exit_code,
                format!("{unit_id}: {}", restarted.failure()),
            )),
            Err(error) => Some((STEP_FAILED, format!("{unit_id}: {error}"))),
        };
        if let Some((code, detail)) = failure {
            steps.push(step_entry(
                "restart",
                host_channel::FAILED_STATUS,
                Some(detail.clone()),
            ));
            failures.push((code, detail));
        }
    }
    (unit_processes, failures)
}
