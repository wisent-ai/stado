//! What the edge serves: the routes the declarations ask for, the directives
//! they render into, the configuration file itself, its delivery, and the
//! report read back off it.

use std::collections::BTreeMap;

use super::{unit_label, CmdError};
use crate::config;

mod caddyfiles;
mod delivery;
mod diagnostics;
mod directives;

pub(in crate::cli::web) use caddyfiles::{caddyfile, terminated_hostnames};
pub(in crate::cli::web) use delivery::deliver;
pub(in crate::cli::web::edge) use diagnostics::{hostnames, status};
pub(in crate::cli::web) use directives::{mount, redirect, route};

/// The site addresses and upstreams the edge must serve, from the product
/// declarations.
///
/// Only the products that name this edge: a `cloudflare` product's hostname is
/// terminated by Cloudflare, and writing it here would order a second
/// certificate for a name this host never answers on.
pub(in crate::cli::web) async fn stado_routes() -> Result<Vec<(String, Vec<String>)>, CmdError> {
    let products = match config::web_api_products() {
        Ok(products) => products,
        // An empty plane is not a broken one; the parser refuses an empty map
        // so a half-written section cannot pass, and "nothing declared" has to
        // read as nothing declared.
        Err(_) if crate::config_file::get("web_api.products").is_none() => return Ok(Vec::new()),
        Err(problems) => return Err(CmdError::click(problems.join("; "))),
    };
    // One site block per hostname, and a hostname can now carry more than one
    // directive: every mount, then the declaration that owns the hostname.
    // Ordered, not sorted, because order is the semantics — Caddy takes the
    // first matching route inside a block, so a catch-all `reverse_proxy`
    // rendered before a `handle_path /docs*` would answer `/docs` itself and
    // the mount would never be reached.
    let mut mounts: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    let mut owners: BTreeMap<&str, String> = BTreeMap::new();
    for product in products
        .values()
        .filter(|product| product.edge() == "stado")
    {
        if let Some(prefix) = product.path_prefix() {
            let (_, directive) = mount(product.hostname(), prefix, product.host(), product.port())?;
            mounts
                .entry(product.hostname())
                .or_default()
                .push((prefix.to_string(), directive));
            continue;
        }
        let (_, directive) = match (product.redirect_to(), product.upstream_service()) {
            (Some(target), _) => redirect(product.hostname(), target)?,
            // Resolved on every render rather than snapshotted at declare
            // time: the service directory is what says which host a service
            // is active on, and a copy of that in this declaration would
            // point at the old host the day the service moved.
            (None, Some(service)) => upstream_route(product.hostname(), service).await?,
            (None, None) => route(product.hostname(), product.host(), product.port())?,
        };
        owners.insert(product.hostname(), directive);
    }
    let mut routes: Vec<(String, Vec<String>)> = Vec::new();
    for (hostname, directive) in owners {
        let mut block = Vec::new();
        if let Some(mounted) = mounts.remove(hostname) {
            // Longest prefix first, so `/docs/api` cannot be swallowed by a
            // `/docs` mounted beside it.
            let mut mounted = mounted;
            mounted
                .sort_by(|left, right| right.0.len().cmp(&left.0.len()).then(left.0.cmp(&right.0)));
            block.extend(mounted.into_iter().map(|(_, directive)| directive));
        }
        block.push(directive);
        routes.push((hostname.to_string(), block));
    }
    // A mount whose owner names another edge. The configuration plane
    // refuses a mount with no owner at all, so what is left here is a mount
    // whose owner is terminated elsewhere, and rendering it on this edge
    // would order a certificate for a name this host never answers on. The
    // first one is named; a run that fixes it will find the next.
    if let Some((hostname, mounted)) = mounts.into_iter().next() {
        let prefixes: Vec<String> = mounted.into_iter().map(|(prefix, _)| prefix).collect();
        return Err(CmdError::click(format!(
            "{} is mounted on {hostname}, whose owning declaration does not name the stado edge: a mount is rendered inside its owner's site block, so it can only be published where the owner is",
            prefixes.join(", ")
        )));
    }
    routes.sort();
    Ok(routes)
}

/// One route to a service the registry already runs: the hostname, and a
/// `reverse_proxy` at wherever the service directory says that service is
/// answering now.
///
/// The port comes out of the directory's endpoint URL and the host out of its
/// `active_host`. Neither is in the declaration, because both are facts about
/// the service rather than about this hostname, and the directory is where
/// the fleet keeps them.
///
/// A loopback endpoint means loopback on the active host, which is exactly
/// what the edge must forward to over the tailnet — Brama answers
/// `127.0.0.1:18081` on the mini and nothing outside that machine can reach
/// it, which is why a public hostname needed the edge in the first place.
async fn upstream_route(hostname: &str, service: &str) -> Result<(String, String), CmdError> {
    let (document, _) = crate::cli::registry::fetch_versioned_document().await?;
    let directory = crate::service_resolution::directory(&document)
        .map_err(CmdError::click)?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{hostname} is declared in front of service {service:?}, and the registry carries no service directory to resolve it through"
            ))
        })?;
    let declared = directory.services.get(service).ok_or_else(|| {
        CmdError::click(format!(
            "{hostname} is declared in front of service {service:?}, which the service directory does not declare; `stado service list` names the ones it does"
        ))
    })?;
    let endpoint = declared
        .endpoints
        .get(&declared.active_host)
        .ok_or_else(|| {
            CmdError::click(format!(
                "service {service:?} declares no endpoint on its active host {:?}, so {hostname} has no upstream the edge can forward to",
                declared.active_host
            ))
        })?;
    let parsed = url::Url::parse(&endpoint.url).map_err(|error| {
        CmdError::click(format!(
            "service {service:?} declares endpoint {:?} on {}, which is not a URL: {error}",
            endpoint.url, declared.active_host
        ))
    })?;
    let port = parsed.port_or_known_default().ok_or_else(|| {
        CmdError::click(format!(
            "service {service:?} declares endpoint {:?}, which names no port the edge could forward to",
            endpoint.url
        ))
    })?;
    route(hostname, &declared.active_host, port)
}
