//! A directory address that names the very resolver socket which is supposed
//! to reach it. Left in place, a caller asks the resolver for a service and
//! the resolver answers with its own address, so the request never leaves the
//! host it was already on.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde_json::Value;

use super::validate::targets;
use super::{directory, ResolverConfig};

/// One directory address that names the resolver socket which is supposed to
/// reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfReferencingEndpoint {
    /// Registry target whose resolver owns the socket.
    pub target: String,
    /// Logical service whose route carries the address.
    pub service: String,
    /// `endpoints` or `standby`: which map on the route holds it.
    pub map: &'static str,
    /// The address both sides name, as the resolver binds it.
    pub address: String,
    /// What the resolver serves on that socket. It is not necessarily
    /// `service`: an adapter for a different service, or the resolver's own
    /// API, on the same port closes the same loop.
    pub adapter: String,
}

/// Normalize an address both sides spell differently — a bind is
/// `127.0.0.1:17614`, an endpoint is `http://127.0.0.1:17614` — so the
/// comparison is between sockets rather than between strings.
pub(super) fn socket_of(url: &str) -> Option<std::net::SocketAddr> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.parse::<IpAddr>().ok()?;
    Some(std::net::SocketAddr::new(
        host,
        parsed.port_or_known_default()?,
    ))
}

/// Every place the directory tells a host to reach a service at the very socket
/// that host's resolver publishes for it.
///
/// An adapter exists to forward a loopback port to wherever the service actually
/// runs. When the endpoint for that host is the adapter's own bind, forwarding
/// has no destination: the adapter dials itself, and the service can only start
/// if it is already running. That is not hypothetical arithmetic — it is what
/// took every `stado host ...` command on the fleet down when the resolver
/// bootstrapped its storage through its own port, and the same shape is sitting
/// in the canonical document right now for `stado-object-api` on
/// `operator-host`.
///
/// Reported rather than rejected, deliberately. [`validate_registry_contract`]
/// runs on every snapshot a resolver loads (`targets::validate_registry` ->
/// `cli::resolver`), so turning this into a validation error would make every
/// host on the fleet refuse the whole directory over a route it is not even
/// using — the failure mode `ServiceRoute::verify` records, at fleet scale.
/// `stado registry doctor` is where it fails instead, with a non-zero exit and
/// the offending pair named. Once the canonical document carries none of these,
/// this becomes a rejection in `validate_resolver_config` and nothing else has
/// to change.
pub fn self_referencing_endpoints(
    document: &Value,
) -> Result<Vec<SelfReferencingEndpoint>, String> {
    let Some(directory) = directory(document)? else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for (target_name, target) in targets(document)? {
        let Some(value) = target.get("service_resolver") else {
            continue;
        };
        // A malformed resolver block is `validate_resolver_config`'s finding to
        // report; skipping it here keeps one diagnosis per defect.
        let Ok(config) = serde_json::from_value::<ResolverConfig>(value.clone()) else {
            continue;
        };
        let mut sockets: BTreeMap<std::net::SocketAddr, String> = BTreeMap::new();
        if let Ok(api) = config.api_bind.parse() {
            sockets.insert(api, "the resolver's own API".to_string());
        }
        for adapter in &config.adapters {
            if let Ok(bind) = adapter.bind.parse() {
                sockets.insert(bind, format!("adapter for {}", adapter.service));
            }
        }
        for (service, route) in &directory.services {
            for (map, endpoints) in [("endpoints", &route.endpoints), ("standby", &route.standby)] {
                let Some(endpoint) = endpoints.get(&target_name) else {
                    continue;
                };
                let Some(socket) = socket_of(&endpoint.url) else {
                    continue;
                };
                if let Some(adapter) = sockets.get(&socket) {
                    found.push(SelfReferencingEndpoint {
                        target: target_name.clone(),
                        service: service.clone(),
                        map,
                        address: socket.to_string(),
                        adapter: adapter.clone(),
                    });
                }
            }
        }
    }
    Ok(found)
}
