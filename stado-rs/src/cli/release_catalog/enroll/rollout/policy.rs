//! The rollout policy document of one product: built from the registry's
//! target entry, the fleet's existing blue-green strategy and the manifest's
//! runtime, with the next two candidate ports no policy binds.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::release_control::DEFAULT_REPLACE_READINESS_PATH;
use crate::release_pipeline::RuntimeContract;

/// The policy document, from the registry and the manifest's runtime.
pub(super) fn policy_for(
    document: &Value,
    product: &str,
    runtime: &RuntimeContract,
    host: &str,
    port: u16,
    one_unit: bool,
) -> Result<Value, CmdError> {
    let target = document
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
        })
        .ok_or_else(|| {
            CmdError::click(format!("registry has no target {host:?}"))
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let platform = target
        .get("release_platform")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::click(format!(
                "registry target {host} declares no release_platform"
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let user = target
        .get("ssh")
        .and_then(Value::as_str)
        .and_then(|ssh| ssh.split_once('@'))
        .map(|(user, _)| user.to_string())
        .ok_or_else(|| {
            CmdError::click(format!("registry target {host} declares no ssh user"))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let home = if user == "root" {
        "/root".to_string()
    } else if platform.starts_with("darwin") {
        format!("/Users/{user}")
    } else {
        format!("/home/{user}")
    };
    let mut strategy = document
        .pointer("/release_control/products")
        .and_then(Value::as_object)
        .and_then(|products| {
            products.values().find_map(|policy| {
                policy
                    .get("strategy")
                    .filter(|strategy| strategy.get("kind").and_then(Value::as_str) == Some("blue-green"))
                    .cloned()
            })
        })
        .ok_or_else(|| {
            CmdError::click("no existing blue-green policy in release_control to take the rollout strategy from")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let (stable_bind, candidate_ports) = if one_unit {
        strategy["kind"] = Value::from("replace");
        (Value::Null, Value::Null)
    } else {
        let (first, second) = free_candidate_ports(document, port)?;
        (
            Value::from(format!("127.0.0.1:{port}")),
            json!([first, second]),
        )
    };
    let prefix = product.to_uppercase().replace('-', "_");
    Ok(json!({
        "service": product,
        "config_schema": runtime.config_schema,
        "state_schema": runtime.state_schema,
        "install_root": format!("{{home}}/.stado/services/{product}"),
        "binary": runtime.binary,
        "launcher": runtime.launcher,
        "binary_env": format!("{prefix}_BIN"),
        "port_env": format!("{prefix}_PORT"),
        "runtime_env": format!("{prefix}_RUNTIME_DIR"),
        "environment": Map::new(),
        "signing_key_item": "",
        "signing_key_id": "",
        "strategy": strategy,
        "targets": {
            host: {
                "platform": platform,
                "run_as_user": user,
                "home": home,
                "state_dir": format!("{home}/.stado/release-state"),
                "runtime_root": format!("{home}/.stado/run"),
                "logs_root": format!("{home}/.stado/logs"),
                "stable_bind": stable_bind,
                "candidate_ports": candidate_ports,
                "readiness_path": runtime.readiness_path.clone().unwrap_or_else(|| DEFAULT_REPLACE_READINESS_PATH.to_string()),
                "legacy_launchd_label": null,
                "legacy_launchd_plist": null,
            }
        },
        "desired": null,
        "previous": null,
    }))
}

/// The two lowest ports above every port a release policy already binds.
///
/// A port a policy declares but that does not read as one is refused with
/// its product and target: skipped, it could be the highest port in use, and
/// the candidates chosen above the rest would land on it.
fn free_candidate_ports(document: &Value, stable: u16) -> Result<(u16, u16), CmdError> {
    let mut used: BTreeSet<u16> = BTreeSet::from([stable]);
    let policies = document
        .pointer("/release_control/products")
        .and_then(Value::as_object)
        .into_iter()
        .flatten();
    for (product, policy) in policies {
        let targets = policy
            .get("targets")
            .and_then(Value::as_object)
            .into_iter()
            .flatten();
        for (host, target) in targets {
            let unreadable = |what: &str, value: &Value| {
                CmdError::refused(format!(
                    "release_control.products.{product}.targets.{host}.{what} is {value}, which \
                     names no port, so the ports already in use are not known; nothing was \
                     enrolled"
                ))
            };
            // `null` is what a replace-strategy policy declares: no ports.
            if let Some(ports) = target
                .get("candidate_ports")
                .filter(|ports| !ports.is_null())
            {
                for port in ports
                    .as_array()
                    .ok_or_else(|| unreadable("candidate_ports", ports))?
                {
                    let read = port.as_u64().and_then(|port| u16::try_from(port).ok());
                    used.insert(read.ok_or_else(|| unreadable("candidate_ports", port))?);
                }
            }
            if let Some(bind) = target.get("stable_bind").filter(|bind| !bind.is_null()) {
                let read = bind
                    .as_str()
                    .and_then(|bind| bind.rsplit_once(':'))
                    .and_then(|(_, port)| port.parse::<u16>().ok());
                used.insert(read.ok_or_else(|| unreadable("stable_bind", bind))?);
            }
        }
    }
    let highest = used.iter().next_back().copied().unwrap_or(stable);
    let first = highest.checked_add(1);
    let second = first.and_then(|port| port.checked_add(1));
    match (first, second) {
        (Some(first), Some(second)) => Ok((first, second)),
        _ => Err(CmdError::unreachable(
            "no free candidate ports above the ports release policies already use",
        )),
    }
}
