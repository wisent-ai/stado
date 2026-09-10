//! The offline half of the public-origin contract: everything a document can
//! be judged wrong about without asking the network.
//!
//! Every reader runs this, and so does every write, so it must never make a
//! request. The two substantive judgements it can still make are the ones a
//! document contradicts on its own: a hostname that is one of a target's
//! declared control-route destinations, and a tailnet name whose node label is
//! not the target that publishes it.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{MAX_PATHS, POLICY_KEY, PUBLICATIONS};
use crate::targets::{ssh_hostname, ComputeTarget};

/// Suffix of a tailnet MagicDNS name, matching [`crate::remote::tailnet`].
const MAGICDNS_SUFFIX: &str = ".ts.net";

/// Loopback origins a publication may forward to. A public origin's upstream
/// is on the target's own loopback by construction: the publication is what
/// crosses the boundary, and an upstream reachable from anywhere else would be
/// a second, undeclared entrance.
const LOOPBACK_HOSTS: &[&str] = &["127.0.0.1", "localhost", "[::1]"];

fn refuse(location: &str, message: &str) -> String {
    format!("{location} {message}")
}

/// Refuse a `public_origins` block that contradicts itself or the document.
///
/// An absent key is valid: a fleet may publish nothing publicly.
pub fn validate_registry_contract(document: &Value) -> Result<(), String> {
    let Some(value) = document.get(POLICY_KEY) else {
        return Ok(());
    };
    let rows = value
        .as_array()
        .ok_or_else(|| refuse(&format!("registry.{POLICY_KEY}"), "must be an array"))?;
    let targets = declared_targets(document);
    let control_routes = control_route_hosts(document);
    let mut names: BTreeSet<&str> = BTreeSet::new();
    let mut hostnames: BTreeSet<&str> = BTreeSet::new();
    for (index, row) in rows.iter().enumerate() {
        let location = format!("registry.{POLICY_KEY}[{index}]");
        let row = row
            .as_object()
            .ok_or_else(|| refuse(&location, "must be an object"))?;
        for key in row.keys() {
            if !matches!(
                key.as_str(),
                "name" | "hostname" | "target" | "publication" | "upstream" | "paths"
            ) {
                return Err(refuse(
                    &location,
                    &format!("contains unsupported key {key:?}"),
                ));
            }
        }

        let name = string(row, &location, "name")?;
        if !is_origin_name(name) {
            return Err(refuse(
                &format!("{location}.name"),
                "must be a lowercase identifier of letters, digits and single hyphens",
            ));
        }
        if !names.insert(name) {
            return Err(refuse(
                &format!("{location}.name"),
                &format!("duplicate public origin name {name:?}"),
            ));
        }

        let hostname = string(row, &location, "hostname")?;
        if !is_public_hostname(hostname) {
            return Err(refuse(
                &format!("{location}.hostname"),
                "must be a bare lowercase DNS name with at least two labels, and no scheme, port or path",
            ));
        }
        if !hostnames.insert(hostname) {
            return Err(refuse(
                &format!("{location}.hostname"),
                &format!("duplicate public hostname {hostname:?}; one hostname is one origin"),
            ));
        }

        let target = string(row, &location, "target")?;
        if !targets.contains(target) {
            return Err(refuse(
                &format!("{location}.target"),
                &format!("names no declared target: {target:?}"),
            ));
        }

        // `/docs/channels`: a host-control route, a service endpoint and a
        // public download origin are separate choices, and a release client
        // does not derive the public origin from a host's control route. A
        // document that names the same destination for both has made that
        // derivation permanent, and every reader would repeat it.
        if control_routes.contains(hostname) {
            return Err(refuse(
                &format!("{location}.hostname"),
                &format!(
                    "{hostname} is a declared host-control destination; a public origin is a \
                     separate choice from the route Stado reaches the host on and must not be \
                     derived from it"
                ),
            ));
        }

        let publication = string(row, &location, "publication")?;
        if !PUBLICATIONS.contains(&publication) {
            return Err(refuse(
                &format!("{location}.publication"),
                &format!("must be one of {}", PUBLICATIONS.join(", ")),
            ));
        }

        // A funnel publishes the node's OWN name. Declaring one host's tailnet
        // name against another host's publication would converge handlers onto
        // a machine that can never serve that SNI, and the first public read
        // would be what discovered it.
        if let Some(node) = hostname.strip_suffix(MAGICDNS_SUFFIX) {
            let label = match node.split('.').next() {
                Some(label) => label,
                None => node,
            };
            if label != target {
                return Err(refuse(
                    &format!("{location}.hostname"),
                    &format!(
                        "{hostname} is the tailnet name of node {label:?}, which is not the \
                         declared target {target:?}; a funnel publishes only its own node's name"
                    ),
                ));
            }
        }

        let upstream = string(row, &location, "upstream")?;
        if !is_loopback_origin(upstream) {
            return Err(refuse(
                &format!("{location}.upstream"),
                "must be a loopback origin like \"http://127.0.0.1:8765\", with no path or trailing slash",
            ));
        }

        validate_paths(row, &location)?;
    }
    Ok(())
}

fn string<'a>(
    row: &'a serde_json::Map<String, Value>,
    location: &str,
    key: &str,
) -> Result<&'a str, String> {
    row.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| refuse(&format!("{location}.{key}"), "must be a non-empty string"))
}

fn validate_paths(row: &serde_json::Map<String, Value>, location: &str) -> Result<(), String> {
    let paths = row
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| refuse(&format!("{location}.paths"), "must be an array"))?;
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(refuse(
            &format!("{location}.paths"),
            &format!("must name between 1 and {MAX_PATHS} paths"),
        ));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (index, path) in paths.iter().enumerate() {
        let field = format!("{location}.paths[{index}]");
        let path = path
            .as_str()
            .ok_or_else(|| refuse(&field, "must be a string"))?;
        if !is_published_path(path) {
            return Err(refuse(
                &field,
                "must be an absolute path with no trailing slash, no empty segment and no \"..\"",
            ));
        }
        if !seen.insert(path) {
            return Err(refuse(&field, &format!("duplicate path {path:?}")));
        }
    }
    Ok(())
}

fn declared_targets(document: &Value) -> BTreeSet<&str> {
    let Some(targets) = document.get("targets").and_then(Value::as_array) else {
        return BTreeSet::new();
    };
    targets
        .iter()
        .filter_map(|target| target.get("name").and_then(Value::as_str))
        .collect()
}

/// Every host name a target declares as a control-route destination, with any
/// `user@` prefix removed.
///
/// The destinations come from [`ComputeTarget::ssh_connections`], which is the
/// one place that knows every ordered route a target declares, so a route kind
/// added there is judged here without this file learning about it. A row that
/// does not deserialize is skipped: the target validator has already refused
/// it with its own sentence, and reporting it twice in two vocabularies is how
/// an operator ends up repairing the wrong field.
fn control_route_hosts(document: &Value) -> BTreeSet<String> {
    let mut hosts = BTreeSet::new();
    let Some(targets) = document.get("targets").and_then(Value::as_array) else {
        return hosts;
    };
    for row in targets {
        let Ok(target) = serde_json::from_value::<ComputeTarget>(row.clone()) else {
            continue;
        };
        for (_, destination) in target.ssh_connections() {
            hosts.insert(ssh_hostname(destination));
        }
    }
    hosts
}

fn is_origin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A bare public DNS name: lowercase, at least two labels, no scheme, port,
/// path or userinfo, and not an address literal. An address literal is refused
/// because a certificate for a public origin is issued for a NAME.
fn is_public_hostname(hostname: &str) -> bool {
    if hostname.len() > 253 || hostname.ends_with('.') {
        return false;
    }
    if hostname.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let labels: Vec<&str> = hostname.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn is_loopback_origin(upstream: &str) -> bool {
    let Some(authority) = upstream.strip_prefix("http://") else {
        return false;
    };
    if authority.contains('/') {
        return false;
    }
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    LOOPBACK_HOSTS.contains(&host) && port.parse::<u16>().is_ok_and(|port| port > 0)
}

fn is_published_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && !path.ends_with('/')
        && !path.contains("//")
        && !path.split('/').any(|segment| segment == "..")
        && !path.contains('?')
        && !path.contains('#')
        && path.is_ascii()
}
