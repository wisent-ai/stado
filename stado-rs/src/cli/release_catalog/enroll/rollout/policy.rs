//! The rollout policy document of one product: built from the registry's
//! target entry, the fleet's existing blue-green strategy and the manifest's
//! runtime, with the next two candidate ports no policy binds.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::release_control::DEFAULT_REPLACE_READINESS_PATH;
use crate::release_pipeline::RuntimeContract;

/// The policy document, from the registry and the manifest's runtime.
/// `home` is the host's own stated home (host_channel::remote_home), never
/// one derived from its SSH user name.
pub(super) fn policy_for(
    document: &Value,
    product: &str,
    runtime: &RuntimeContract,
    host: &str,
    home: &str,
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
            // The stable bind is this host's loopback at the declared port.
            Value::from(
                std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)).to_string(),
            ),
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
                    let number = port
                        .as_u64()
                        .ok_or_else(|| unreadable("candidate_ports", port))?;
                    used.insert(u16::try_from(number).map_err(|error| {
                        CmdError::refused(format!(
                            "release_control.products.{product}.targets.{host}.candidate_ports \
                             holds {number}, which is not a port ({error}); nothing was enrolled"
                        ))
                    })?);
                }
            }
            if let Some(bind) = target.get("stable_bind").filter(|bind| !bind.is_null()) {
                let text = bind
                    .as_str()
                    .ok_or_else(|| unreadable("stable_bind", bind))?;
                let (_, port) = text
                    .rsplit_once(':')
                    .ok_or_else(|| unreadable("stable_bind", bind))?;
                used.insert(port.parse::<u16>().map_err(|error| {
                    CmdError::refused(format!(
                        "release_control.products.{product}.targets.{host}.stable_bind {text:?} \
                         names no port ({error}); nothing was enrolled"
                    ))
                })?);
            }
        }
    }
    let highest = used.into_iter().fold(stable, u16::max);
    // The ports above the highest one in use, lowest first.
    let mut free = (highest..=u16::MAX).filter(|port| *port > highest);
    match (free.next(), free.next()) {
        (Some(first), Some(second)) => Ok((first, second)),
        _ => Err(CmdError::unreachable(
            "no free candidate ports above the ports release policies already use",
        )),
    }
}
