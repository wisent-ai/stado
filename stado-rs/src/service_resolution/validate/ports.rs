//! One host, one owner per loopback port.
//!
//! Split out of `validate/contract.rs`, which had grown past the module line
//! cap; the rest of the whole-document check stays there and calls this.

use std::collections::BTreeMap;

use serde_json::Value;

/// One host, one owner per loopback port.
///
/// Two declarations in this same document claim ports on the same host: a
/// blue-green release target names the stable bind its proxy must own and the
/// two ports its candidates alternate between, and that host's resolver names
/// a bind per adapter. Nothing compared them, and on 2026-09-20 they
/// overlapped: `lukasz-macbook`'s resolver served the `weles-admission`
/// adapter for consumer `skarbiec-weles-credential-client` on
/// `127.0.0.1:18787`, which is Skarbiec's release stable bind. Every rollout
/// of that release spawned a candidate, the candidate died on `Address
/// already in use (os error 48)`, the digest was quarantined, and the fleet's
/// credential plane stayed down for three days while both declarations read
/// correct on their own.
pub(super) fn refuse_release_port_collisions(
    document: &Value,
    target_entries: &BTreeMap<String, &Value>,
) -> Result<(), String> {
    let Some(control) = crate::release_control::control(document)? else {
        return Ok(());
    };
    for (target_name, target) in target_entries {
        let Some(value) = target.get("service_resolver") else {
            continue;
        };
        let Ok(resolver) =
            serde_json::from_value::<crate::service_resolution::ResolverConfig>(value.clone())
        else {
            // A malformed resolver block is `validate_resolver_config`'s
            // finding; one diagnosis per defect.
            continue;
        };
        let mut claimed: BTreeMap<u16, String> = BTreeMap::new();
        for adapter in &resolver.adapters {
            if let Some(port) = socket_port(&adapter.bind) {
                claimed.insert(port, format!("resolver adapter {}", adapter.service));
            }
        }
        if let Some(port) = socket_port(&resolver.api_bind) {
            claimed.insert(port, "resolver api_bind".to_string());
        }
        for (product, policy) in &control.products {
            let Some(release) = policy.targets.get(target_name.as_str()) else {
                continue;
            };
            let mut release_ports: Vec<(u16, &'static str)> = Vec::new();
            if let Some(port) = release.stable_bind.as_deref().and_then(socket_port) {
                release_ports.push((port, "stable_bind"));
            }
            for port in release.candidate_ports.into_iter().flatten() {
                release_ports.push((port, "candidate port"));
            }
            for (port, what) in release_ports {
                if let Some(claimant) = claimed.get(&port) {
                    return Err(format!(
                        "registry.targets[{target_name}].service_resolver: {claimant} claims \
                         127.0.0.1:{port}, which registry.release_control.products.{product} \
                         declares as its {what} on this host; one loopback port has one owner"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The port of a `host:port` declaration, when it names one.
fn socket_port(bind: &str) -> Option<u16> {
    bind.rsplit(':').next()?.parse().ok()
}
