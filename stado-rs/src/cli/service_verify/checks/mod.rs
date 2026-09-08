//! Which declarations are checked, from where, and at what address.

pub(in crate::cli::service_verify) mod local;
pub(in crate::cli::service_verify) mod standby;

use std::collections::BTreeSet;

use serde_json::Value;

use crate::targets::{
    Service, VerifyDescriptor, VERIFY_FROM_ACTIVE_HOST, VERIFY_FROM_ENDPOINT_HOLDERS,
    VERIFY_KIND_HTTP,
};

/// Which hosts probe this service?
///
/// [`VERIFY_FROM_ENDPOINT_HOLDERS`], the default, is every host the directory
/// hands a dial address to, plus the active host. That map is what a consumer
/// actually reads: `service directory publish` writes `endpoints[<this host>]`
/// into `~/.stado/forwards/<service>.local` and skips a service that gives
/// this host no entry. So a host carrying an endpoint has been handed an
/// address and can be held to it, whether or not it is the one serving.
///
/// A host that only stands by holds no entry in that map -- its address lives
/// in `Service::standby`, is reported by [`standby_findings`](standby::standby_findings), and is never
/// probed from anywhere.
///
/// [`VERIFY_FROM_ACTIVE_HOST`] is only where the service claims to serve, for
/// an endpoint no other host was ever meant to reach. Probing that from four
/// vantages files four `unreachable` rows against a service working exactly as
/// declared, and a report that cries wolf gets read like one.
///
/// An unrecognized vantage keeps the wider set on purpose, so no declaration
/// vanishes from the table; those rows come back `unverified` naming the
/// vantage. A missing row reads as "fine" to every operator alive.
///
/// Deliberately NOT `consumers`. That map is keyed by consumer identity -- the
/// name of the software calling in, like `weles` -- and not by host, so reading
/// it as a host set produces confident answers about machines that do not
/// exist. The consumer-to-host mapping lives in placement, not here, and
/// guessing at it would put this command in the same class of defect it was
/// written to catch.
pub(in crate::cli::service_verify) fn probe_hosts(
    service: &Service,
    descriptor: &VerifyDescriptor,
) -> BTreeSet<String> {
    let endpoint_holders = || {
        let mut hosts: BTreeSet<String> = service.endpoints.keys().cloned().collect();
        hosts.insert(service.active_host.clone());
        hosts
    };
    match descriptor.from.as_str() {
        VERIFY_FROM_ACTIVE_HOST => BTreeSet::from([service.active_host.clone()]),
        VERIFY_FROM_ENDPOINT_HOLDERS => endpoint_holders(),
        _ => endpoint_holders(),
    }
}

/// The descriptor asks for something this build cannot do, spelled out for the
/// row it will produce.
///
/// This is the registry validator's own function, deliberately. One list of
/// implemented values means a descriptor cannot pass validation and then find
/// no prober, nor be refused by a prober the validator was happy with -- two
/// lists is how a declaration ends up with a reader that does not exist.
fn unsupported(service: &str, descriptor: &VerifyDescriptor) -> Option<String> {
    let problems = crate::targets::validate_verification(service, descriptor);
    if problems.is_empty() {
        return None;
    }
    Some(problems.join("; "))
}

/// The address a given host is told to call.
///
/// [`Service::address_for`], never the standby map: this feeds the prober,
/// and a standby address is declared to have nothing listening on it.
///
/// The health path is appended for `http` and withheld for `tcp`: a path is
/// not something you can send down a socket, and pasting one onto an address
/// produces a port nobody is listening on. An endpoint with no entry for a
/// host that is supposed to call it is itself a finding: the consumer has been
/// authorized and given no address.
pub(in crate::cli::service_verify) fn endpoint_for(
    service: &Service,
    host: &str,
    kind: &str,
) -> Option<String> {
    let endpoint = service.address_for(host)?;
    if kind != VERIFY_KIND_HTTP {
        return Some(endpoint.url.clone());
    }
    let health = endpoint
        .extra
        .get("health")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if health.is_empty() {
        return Some(endpoint.url.clone());
    }
    Some(format!(
        "{}/{}",
        endpoint.url.trim_end_matches('/'),
        health.trim_start_matches('/')
    ))
}
