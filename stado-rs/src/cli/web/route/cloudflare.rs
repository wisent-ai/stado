//! The other edge: a hostname that declares the `cloudflare` edge is carried
//! by the Cloudflare tunnel its connector host already runs. Nothing on the
//! public internet has to reach the fleet for it: no router mapping, no public
//! address, no edge VM. Cloudflare terminates TLS and the connector carries
//! the request out to the origin on its own host.
//!
//! `stado web route` publishes such a hostname with the same operation
//! `stado tunnel route --provider cloudflare` runs: ingress first, then the
//! connector token, then the proxied DNS record. The credentials are the
//! items playing [`API_ROLE`] and [`TUNNEL_ROLE`] in the owner vault, read by
//! role as every other fleet credential is, so no item name is written into
//! a declaration or chosen here.

use serde_json::json;

use super::planning::zone_of;
use super::CmdError;
use crate::config::WebApiProduct;
use crate::primitives::failure::FailureCode;

/// The role of the vault item holding the Cloudflare `account_id` and the
/// scoped `api_token` that edits tunnel ingress and DNS.
const API_ROLE: &str = "cloudflare-api";

/// The role of the vault item holding the same `account_id`, the `tunnel_id`
/// and the connector token the connector host runs the tunnel with.
const TUNNEL_ROLE: &str = "cloudflare-tunnel";

/// Publish one `cloudflare`-edge hostname through the tunnel.
///
/// With `check`, everything is resolved — the zone, the connector host and
/// origin, both credential items — and nothing is changed: the plan is
/// printed. A mount or a redirect is refused by name, because the tunnel
/// route carries one hostname to one origin and has no path or redirect rule.
pub(super) async fn publish(
    declared: &WebApiProduct,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let hostname = declared.hostname();
    if let Some(prefix) = declared.path_prefix() {
        return Err(CmdError::refused(format!(
            "{hostname}{prefix} is a mount on the cloudflare edge; a tunnel route carries a whole \
             hostname to one origin, so mount it on a hostname that declares the stado edge"
        )));
    }
    if let Some(target) = declared.redirect_to() {
        return Err(CmdError::refused(format!(
            "{hostname} redirects to {target} on the cloudflare edge; a tunnel route carries a \
             hostname to an origin and renders no redirect, so declare the redirect on the stado \
             edge or as a Cloudflare redirect rule"
        )));
    }
    let zone = zone_of(hostname);
    let (connector_host, origin) = connector_origin(declared).await?;
    let (owner, _) = crate::cli::release_catalog::fleet_hosts().await?;
    let api_item = role_item(&owner, API_ROLE).await?;
    let tunnel_item = role_item(&owner, TUNNEL_ROLE).await?;
    if check {
        let plan = json!({
            "hostname": hostname,
            "edge": "cloudflare",
            "zone": zone,
            "origin": origin,
            "connector_host": connector_host,
            "connector_service": crate::cli::cloudflare::CONNECTOR_SERVICE,
            "api_credential": api_item,
            "tunnel_credential": tunnel_item,
            "change": "would-route",
        });
        if json_output {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            println!(
                "{hostname}: would route through the Cloudflare tunnel in zone {zone} to {origin} \
                 on {connector_host} ({api_item} and {tunnel_item} from the vault on {owner})"
            );
        }
        return Ok(());
    }
    crate::cli::cloudflare::route_tunnel(
        &api_item,
        &tunnel_item,
        &zone,
        hostname,
        &origin,
        &connector_host,
        crate::cli::cloudflare::CONNECTOR_SERVICE,
        crate::cli::cloudflare::CONNECTOR_TOKEN_FIELD,
        crate::cli::cloudflare::CONNECTOR_SECRET_NAME,
        json_output,
    )
    .await
}

/// The host the connector carries this hostname from, and the origin it
/// forwards to there.
///
/// A hostname in front of a registry service follows the service directory:
/// the service's active host runs the connector and its endpoint there is the
/// origin, loopback included, since the connector forwards from that host. A
/// hostname declared with a host and port is served by that host's loopback
/// port.
async fn connector_origin(declared: &WebApiProduct) -> Result<(String, String), CmdError> {
    let hostname = declared.hostname();
    let Some(service) = declared.upstream_service() else {
        return Ok((
            declared.host().to_string(),
            format!("http://localhost:{}", declared.port()),
        ));
    };
    let (document, _) = crate::cli::registry::fetch_versioned_document().await?;
    let directory = crate::service_resolution::directory(&document)
        .map_err(CmdError::declaration)?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{hostname} is declared in front of service {service:?}, and the registry carries \
                 no service directory to resolve it through"
            ))
            .stating(FailureCode::Config)
        })?;
    let entry = directory.services.get(service).ok_or_else(|| {
        CmdError::click(format!(
            "{hostname} is declared in front of service {service:?}, which the service directory \
             does not declare; `stado service list` names the ones it does"
        ))
        .stating(FailureCode::NotFound)
    })?;
    let endpoint = entry.endpoints.get(&entry.active_host).ok_or_else(|| {
        CmdError::click(format!(
            "service {service:?} declares no endpoint on its active host {:?}, so {hostname} has \
             no origin the tunnel could forward to",
            entry.active_host
        ))
        .stating(FailureCode::Config)
    })?;
    Ok((entry.active_host.clone(), endpoint.url.clone()))
}

/// The one item in the owner vault playing `role`, or a refusal naming the
/// command that tags it.
async fn role_item(owner: &str, role: &str) -> Result<String, CmdError> {
    crate::cli::host::vault_role_item(owner, role)
        .await?
        .ok_or_else(|| {
            CmdError::click(format!(
                "no item in the owner vault on {owner} plays role {role}; tag the item holding \
                 that Cloudflare credential with `stado credentials item retag --host {owner} \
                 <item> --tags {}`",
                crate::skarbiec::roles::role_tag(role)
            ))
            .stating(FailureCode::NotFound)
        })
}

/// Why `stado web remove` leaves a `cloudflare`-edge hostname routed, and
/// the command that removes it.
pub(super) fn retraction_refused(hostname: &str) -> String {
    let zone = zone_of(hostname);
    format!(
        "{hostname} is carried by the Cloudflare tunnel, which `stado web remove` does not take \
         down; `stado tunnel remove --provider cloudflare --api-credential <item playing \
         {API_ROLE}> --tunnel-credential <item playing {TUNNEL_ROLE}> --zone {zone} --hostname \
         {hostname}` removes its ingress and record"
    )
}
