//! Handing a product's port from its blue-green serving to the product's one
//! unit, after its release policy on this host moved to `replace`.
//!
//! A blue-green product is served by a release proxy inside the host's Stado
//! process, forwarding the stable port to a candidate the agent spawned. When
//! the policy becomes `replace`, the agent stops driving the product, and
//! nothing else ever stopped that proxy or those processes: the product's own
//! unit could never bind its port, so the product kept running outside any
//! unit. This pass gives the port to that unit once the unit is loaded on this
//! host, and only then, so a host never ends up with nothing serving.

use std::process::Command;

use serde_json::Value;

use super::sweep::sweep_leaked_processes;
use crate::release_agent::rollout::serving::control;
use crate::release_agent::rollout::serving::owner::process::terminate;
use crate::release_agent::state::document::{load_state, proxy_state_path, save_state};
use crate::release_agent::state::records::HostReleaseState;
use crate::release_control::ProductReleasePolicy;

/// What happened to `product`'s blue-green serving on `target_name` this
/// pass, or `None` when it has none left.
pub(crate) async fn hand_over_to_unit(
    document: &Value,
    product: &str,
    policy: &ProductReleasePolicy,
    target_name: &str,
) -> Result<Option<String>, String> {
    let Some(target) = policy.targets.get(target_name) else {
        return Ok(None);
    };
    let state = load_state(target, product, target_name)?;
    let records: Vec<_> = [&state.active, &state.candidate, &state.previous]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    if state.proxy_pid.is_none() && records.is_empty() {
        return Ok(None);
    }
    let (unit, bind) = served_unit(document, &policy.service, target_name)?;
    if !unit_loaded(&unit)? {
        return Ok(Some(format!(
            "{product} is still served blue-green on {bind}: its unit {unit} is not loaded on \
             this host, so the port stays with the release proxy until it is"
        )));
    }
    control::stop(
        Some(&target.home),
        &proxy_state_path(target, product),
        &bind,
    )
    .await?;
    for record in &records {
        terminate(record);
    }
    let mut cleared = HostReleaseState::new(product, target_name);
    cleared.rollout_generation = state.rollout_generation;
    cleared.quarantined = state.quarantined;
    sweep_leaked_processes(target, product, &policy.install_root, &cleared);
    cleared.detail = format!(
        "handed {bind} to {unit}: stopped the release proxy and {} blue-green release \
         process(es) ({})",
        records.len(),
        records
            .iter()
            .map(|record| format!("{} pid {} port {}", record.version, record.pid, record.port))
            .collect::<Vec<_>>()
            .join(", ")
    );
    save_state(target, &mut cleared)?;
    Ok(Some(cleared.detail))
}

/// The unit the service directory names for `service` on `target_name`, and
/// the `host:port` its endpoint there serves — the port the proxy held.
fn served_unit(
    document: &Value,
    service: &str,
    target_name: &str,
) -> Result<(String, String), String> {
    let directory = crate::service_resolution::directory(document)?
        .ok_or_else(|| "the registry has no service directory".to_string())?;
    let route = directory.services.get(service).ok_or_else(|| {
        format!("the service directory has no {service:?} for the release policy to replace")
    })?;
    let unit = route
        .managed_service
        .clone()
        .ok_or_else(|| format!("service {service:?} names no managed unit to hand its port to"))?;
    let endpoint = route
        .endpoints
        .get(target_name)
        .ok_or_else(|| format!("service {service:?} has no endpoint on {target_name}"))?;
    let url = url::Url::parse(&endpoint.url)
        .map_err(|error| format!("service {service:?} endpoint {}: {error}", endpoint.url))?;
    let host = url.host_str().ok_or_else(|| {
        format!(
            "service {service:?} endpoint {} names no host",
            endpoint.url
        )
    })?;
    let port = url.port_or_known_default().ok_or_else(|| {
        format!(
            "service {service:?} endpoint {} names no port",
            endpoint.url
        )
    })?;
    Ok((unit, format!("{host}:{port}")))
}

/// Whether the init system on this host holds `unit`: launchd in the login
/// session or the system domain, or a systemd user or system unit that is
/// enabled. A loaded unit that cannot bind yet is exactly what this pass waits
/// for; the init system restarts it once the port is free.
fn unit_loaded(unit: &str) -> Result<bool, String> {
    let asked = |program: &str, args: &[&str]| -> Result<bool, String> {
        Command::new(program)
            .args(args)
            .output()
            .map(|output| output.status.success())
            .map_err(|error| format!("cannot ask {program} about {unit}: {error}"))
    };
    if cfg!(target_os = "macos") {
        let session = format!("gui/{}/{unit}", nix::unistd::getuid());
        let system = format!("system/{unit}");
        return Ok(asked("/bin/launchctl", &["print", &session])?
            || asked("/bin/launchctl", &["print", &system])?);
    }
    let service = if unit.ends_with(".service") {
        unit.to_string()
    } else {
        format!("{unit}.service")
    };
    Ok(asked("systemctl", &["--user", "is-enabled", &service])?
        || asked("systemctl", &["is-enabled", &service])?)
}
