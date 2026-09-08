//! The readers behind three of the four facts: what a hostname resolves to,
//! where it is supposed to point, and what the service plane says about a
//! unit somebody else owns.
//!
//! Each one answers its own question and forms no opinion about the product.
//! The precedence between their answers is `examine`'s, deliberately, so that
//! changing what counts as broken never means editing a reader.

use serde_json::{json, Value};

use super::{DNS_RESOLVED, DNS_TIMEOUT, DNS_UNREADABLE, DNS_UNRESOLVED};
use crate::config::WebApiProduct;
use crate::deploy::service::ServiceStatus;

/// Resolve one public hostname to its addresses.
///
/// This uses `tokio::net::lookup_host` — the host's own stub resolver, through
/// the standard library — rather than a DNS client of this crate's own. There
/// is no resolver machinery here to reuse: `cli/dns.rs` speaks Namecheap's
/// zone API and answers "what does the zone say", which is a different
/// question and would go on answering correctly while a record served from a
/// stale cache pointed somewhere else. `doctor.rs` already reaches for
/// `lookup_host` under a timeout for exactly this reason, and this follows it,
/// so what is reported is what a browser would actually get.
///
/// The port in the query is `443` because `lookup_host` resolves a socket
/// address and needs one; it is discarded, and nothing here connects.
pub(super) async fn resolve_hostname(hostname: &str) -> (&'static str, Vec<String>) {
    let lookup = tokio::time::timeout(
        DNS_TIMEOUT,
        tokio::net::lookup_host(format!("{hostname}:443")),
    )
    .await;
    match lookup {
        Ok(Ok(addresses)) => {
            let mut found: Vec<String> = addresses
                .map(|address| address.ip().to_string())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            found.dedup();
            if found.is_empty() {
                (DNS_UNRESOLVED, found)
            } else {
                (DNS_RESOLVED, found)
            }
        }
        // A resolver that answers "no such name" and a resolver that cannot be
        // reached both arrive here as an error from the same call, and the
        // stub resolver does not distinguish them for us. Reported as
        // unresolved with the reason in `dns_detail`, never as an address.
        Ok(Err(_)) => (DNS_UNRESOLVED, Vec::new()),
        Err(_) => (DNS_UNREADABLE, Vec::new()),
    }
}

/// Where this product's hostname is supposed to point, when that is knowable.
///
/// For the Stado edge it is the edge host's public IPv4, which the
/// configuration plane holds — the record `stado web route` writes is an A
/// record to exactly that address, so an answer that does not carry it is a
/// name pointing at something else.
///
/// For the Cloudflare edge it is `None`, and that is a statement rather than a
/// gap: a proxied record answers with Cloudflare's own anycast addresses,
/// which are theirs to change and not ours to enumerate, so asserting one
/// would produce a false finding every time they rotate. Such a product is
/// judged on whether the name resolves at all, which is the part that is
/// genuinely this fleet's business.
pub(super) fn expected_address(
    declared: &WebApiProduct,
) -> Result<Option<&'static str>, &'static [String]> {
    if declared.edge() != "stado" {
        return Ok(None);
    }
    crate::config::web_api_edge().map(|edge| Some(edge.address()))
}

/// Everything one product's report needs, gathered from the four readers.
/// What the service plane says about the unit behind an upstream hostname.
///
/// Read out of the same fleet-wide beacon join every other row uses, looked
/// up by service name. This command's job here is to report the service
/// plane's answer beside the hostname, not to form a second opinion about a
/// unit somebody else owns.
pub(super) fn upstream_service_state(service: &str, managed: &[ServiceStatus]) -> Value {
    match managed.iter().find(|row| row.service.matches(service)) {
        Some(row) => json!({
            "service": service,
            "host": row.service.host,
            "state": row.state,
            "reported_at": row.reported_at,
        }),
        // Not a verdict about the service: the beacon join simply carries no
        // row for it, which is what an undeclared or never-deployed service
        // looks like from here.
        None => json!({
            "service": service,
            "state": "undeclared",
            "detail": format!("the managed service set carries no row for {service:?}"),
        }),
    }
}
