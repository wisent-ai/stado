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
    let (served, bind) = served_unit(document, &policy.service, target_name)?;
    // The unit the port goes to is the product's one catalog unit — the unit
    // enrollment converted the policy towards. The directory's unit is only
    // the fallback for a product the catalog does not name: it still names
    // the old label until an ensure of the new unit is recorded, and that
    // ensure cannot succeed while the proxy holds the port.
    let unit = crate::deploy::service_catalog::lookup(product)?
        .and_then(|entry| entry.unit)
        .unwrap_or(served);
    if !unit_loaded(&unit)? {
        return Ok(Some(format!(
            "{product} is still served blue-green on {bind}: its unit {unit} is not loaded on \
             this host, so the port stays with the release proxy until it is"
        )));
    }
    let proxy_state = proxy_state_path(target, product);
    let proxy_holds = control::inspect(Some(&target.home), &proxy_state, &bind)
        .await?
        .is_some();
    // Without the proxy on the port, the blue-green processes may end only
    // when somebody else already serves it — the unit, which took a port the
    // proxy no longer held (a host process restarted on a `replace` policy
    // does not start a proxy again). A port nobody listens on means the proxy
    // holds another one, still forwarding to these processes.
    let unit_holds = !proxy_holds
        && bind
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
            .and_then(|port| super::inventory::listener_pid(Some(port)))
            .is_some_and(|holder| !records.iter().any(|record| record.pid == holder));
    if state.proxy_pid.is_some() && !proxy_holds && !unit_holds {
        return Ok(Some(format!(
            "{product} is still served blue-green, but its release proxy does not hold {bind}, \
             the port {unit} serves, and nothing else listens there; nothing was stopped, \
             because ending its processes would leave the port the proxy does hold forwarding \
             to nothing"
        )));
    }
    if proxy_holds {
        control::stop(Some(&target.home), &proxy_state, &bind).await?;
    }
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

/// The unit that serves `service` on `target_name`, and the `host:port` its
/// endpoint there serves — the port the proxy held. A fixed route names the
/// unit as its `managed_service`; a placement-backed route leaves that absent
/// and its placement profile names the unit for each host.
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
    let unit = match (&route.managed_service, &route.placement_profile) {
        (Some(unit), _) => unit.clone(),
        (None, Some(profile)) => document
            .get("placement_profiles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|candidate| candidate.get("name").and_then(Value::as_str) == Some(profile))
            .and_then(|found| found.pointer(&format!("/hosts/{target_name}/units/{service}/unit")))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "placement profile {profile:?} names no unit for {service:?} on {target_name}"
                )
            })?,
        (None, None) => {
            return Err(format!(
                "service {service:?} names neither a managed unit nor a placement profile to \
                 hand its port to"
            ))
        }
    };
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
        crate::wait::output(Command::new(program).args(args))
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

/// Retire what release control left on this host for every product whose
/// policy no longer targets it: the proxy its state file owns, the release
/// processes it launched (only processes running out of the product's
/// release directory with the agent's launch marker — a pid from an old
/// state file may belong to anything by now), and its state and proxy files.
/// `targeted` names the products whose policy lists this host; `home` is the
/// account's home, under which every target keeps `.stado/release-state` and
/// `.stado/services/<product>`. Returns one line per product acted on.
pub(crate) async fn retire_untargeted(
    home: &str,
    targeted: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let state_dir = std::path::Path::new(home).join(".stado/release-state");
    let Ok(entries) = std::fs::read_dir(&state_dir) else {
        return Vec::new();
    };
    let mut products: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix("-proxy.json").map(str::to_string))
        .filter(|product| !targeted.contains(product))
        .collect();
    products.sort();
    let mut lines = Vec::new();
    for product in products {
        let proxy = state_dir.join(format!("{product}-proxy.json"));
        if let Err(error) = control::retire(Some(home), &proxy).await {
            lines.push(format!(
                "{product} is no longer released to this host, but its release proxy could \
                 not be retired: {error}"
            ));
            continue;
        }
        let install_root = format!("{home}/.stado/services/{product}");
        let processes = super::inventory::release_processes(&install_root);
        let spawned: Vec<i32> = processes
            .iter()
            .filter(|process| process.agent_spawned)
            .map(|process| process.process_group)
            .collect();
        let mut ended = Vec::new();
        for process in &processes {
            if process.agent_spawned || spawned.contains(&process.process_group) {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(process.pid),
                    nix::sys::signal::Signal::SIGTERM,
                );
                ended.push(format!(
                    "{} pid {} port {:?}",
                    process.version, process.pid, process.port
                ));
            }
        }
        if !ended.is_empty() {
            // Removed on the pass that finds nothing left running, so a
            // process that ignored this SIGTERM is found and signalled again.
            lines.push(format!(
                "{product} is no longer released to this host: retired its release proxy and \
                 ended {}",
                ended.join(", ")
            ));
            continue;
        }
        let _ = std::fs::remove_file(&proxy);
        let _ = std::fs::remove_file(state_dir.join(format!("{product}.json")));
        lines.push(format!(
            "{product} is no longer released to this host: nothing of it runs here; its \
             release state was removed"
        ));
    }
    lines
}
