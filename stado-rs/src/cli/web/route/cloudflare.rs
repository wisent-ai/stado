//! The other edge: a declaration that chose the `cloudflare` edge is
//! published by `stado tunnel --provider cloudflare`, not by `stado web route`.

use super::planning::zone_of;

/// Why `stado web route` does not publish a `cloudflare`-edge hostname, and
/// the command that does.
///
/// This refusal is the whole arm on purpose. Falling back to the Stado edge
/// would publish the hostname from an edge the declaration did not choose,
/// and picking a credential for the operator is not something a control
/// plane does: the tunnel route names its own items.
pub(super) fn cloudflare_unavailable(hostname: &str) -> String {
    let zone = zone_of(hostname);
    format!(
        "{hostname} declares the cloudflare edge, which `stado web route` does not publish; \
         `stado tunnel route --provider cloudflare --api-credential <item with account_id and \
         api_token> --tunnel-credential <item with account_id and tunnel_id> --zone {zone} \
         --hostname {hostname} --origin <url> --host <connector host>` publishes it. {zone} \
         must be a zone Cloudflare's nameservers serve; `stado dns delegate {zone} \
         --provider cloudflare --api-credential <item>` moves a zone the registrar serves into \
         Cloudflare."
    )
}
