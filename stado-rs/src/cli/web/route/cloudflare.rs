//! The other edge, and why it cannot be exercised: one refusal, written out
//! of what Skarbiec actually holds.

use super::planning::zone_of;

/// Why the Cloudflare edge cannot be exercised, in the vault's own terms.
///
/// Measured, not assumed: `platform-admin-cloudflare` carries `username` and
/// `password` — a console login. `platform-cloudflare-bobloo-tunnel` carries
/// `account_id`, `token`, `tunnel_id` and `tunnel_name`, and its `token` is a
/// 180-character `cloudflared` tunnel token which the Cloudflare API rejects
/// as a bearer with code 6111, `Invalid format for Authorization header`.
/// [`crate::cli::cloudflare`] requires an `--api-credential` item carrying
/// `account_id` and `api_token`, and no item in Skarbiec carries an
/// `api_token` at all.
///
/// This refusal is the whole arm on purpose. Falling back to the Stado edge
/// would publish the hostname from an edge the declaration did not choose, and
/// inventing a credential is not something a control plane does.
pub(super) fn cloudflare_unavailable(hostname: &str) -> String {
    let zone = zone_of(hostname);
    format!(
        "{hostname} declares the cloudflare edge, and Skarbiec holds no item carrying the \
         `api_token` field that `stado cloudflare --api-credential` requires: \
         `platform-admin-cloudflare` carries only a console `username` and `password`, and \
         `platform-cloudflare-bobloo-tunnel` carries `account_id`, `token`, `tunnel_id` and \
         `tunnel_name` — its `token` is a cloudflared tunnel token, which the Cloudflare API \
         refuses as a bearer with code 6111, `Invalid format for Authorization header`. Add a \
         Skarbiec item carrying `account_id` and a scoped `api_token`, and \
         `stado cloudflare route-tunnel --api-credential <that item> --tunnel-credential \
         platform-cloudflare-bobloo-tunnel --zone {zone} --hostname {hostname}` publishes it. \
         {zone} must also be a zone Cloudflare's nameservers serve, because Cloudflare issues \
         that certificate only for a zone it serves; `stado dns delegate {zone} \
         --api-credential <that item>` moves a zone the registrar serves into Cloudflare."
    )
}
