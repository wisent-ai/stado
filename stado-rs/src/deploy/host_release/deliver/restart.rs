use std::collections::BTreeMap;

use serde_json::Value;

use super::super::ReleasePlan;
use super::step_entry;
use crate::deploy::{host_channel, service, service_label_print, Runner};
use crate::targets::ComputeTarget;

/// Restart every declared unit and join each new init-system pid to the image
/// digest activation reported, in the order the units were declared.
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
    } else {
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
            let arguments = std::iter::once(declared.program.as_str())
                .chain(declared.args.iter().map(String::as_str))
                .collect::<Vec<_>>();
            if crate::self_update::defers_to_release_handshake(&arguments) {
                if let Some(evidence) = prior_unit_state.get(&unit_id) {
                    unit_processes.push(evidence.to_json());
                }
                steps.push(step_entry(
                    "restart",
                    "deferred_to_release_handshake",
                    Some(format!(
                        "{unit_id} is the queue agent and will recycle itself after its current slot"
                    )),
                ));
                continue;
            }
            match service::restart_service(target, declared, runner).await {
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
                            let previous = prior_unit_state.get(&unit_id);
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
                                && current.process_inode.is_some_and(|inode| inode != 0)
                                && (plan.product.install.is_tree()
                                    || current.process_sha256.as_deref() == Some(active_sha256));
                            unit_processes.push(current.to_json());
                            if fresh && exact_image {
                                let detail = format!(
                                    "{unit_id} fresh pid {} image {} sha256 {}",
                                    current.pid.as_deref().unwrap_or_default(),
                                    current.process_executable.as_deref().unwrap_or_default(),
                                    current.process_sha256.as_deref().unwrap_or_default()
                                );
                                steps.push(step_entry("restart", "ok", Some(detail)));
                            } else {
                                let detail = format!(
                                    "{unit_id}: restart did not prove a fresh pid on the activated immutable image; prior pid={} start={} current pid={} start={} executable={} device={} inode={} sha256={} identity_unavailable={}",
                                    previous_pid.unwrap_or_default(),
                                    previous_start.unwrap_or_default(),
                                    current_pid.unwrap_or_default(),
                                    current_start.unwrap_or_default(),
                                    current.process_executable.as_deref().unwrap_or_default(),
                                    current.process_device.unwrap_or_default(),
                                    current.process_inode.unwrap_or_default(),
                                    current.process_sha256.as_deref().unwrap_or_default(),
                                    current.process_identity_unavailable.as_deref().unwrap_or_default()
                                );
                                steps.push(step_entry(
                                    "restart",
                                    host_channel::FAILED_STATUS,
                                    Some(detail.clone()),
                                ));
                                failures.push((1, detail));
                            }
                        }
                        Err(error) => {
                            let detail =
                                format!("{unit_id}: process identity read failed: {error}");
                            steps.push(step_entry(
                                "restart",
                                host_channel::FAILED_STATUS,
                                Some(detail.clone()),
                            ));
                            failures.push((1, detail));
                        }
                    }
                }
                Ok(restarted) => {
                    let detail = format!("{unit_id}: {}", restarted.failure());
                    steps.push(step_entry(
                        "restart",
                        host_channel::FAILED_STATUS,
                        Some(detail.clone()),
                    ));
                    failures.push((restarted.exit_code, detail));
                }
                Err(error) => {
                    let detail = format!("{unit_id}: {error}");
                    steps.push(step_entry(
                        "restart",
                        host_channel::FAILED_STATUS,
                        Some(detail.clone()),
                    ));
                    failures.push((1, detail));
                }
            }
        }
    }
    (unit_processes, failures)
}
