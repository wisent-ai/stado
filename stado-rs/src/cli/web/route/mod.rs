//! `stado web route` — putting one declared hostname on the public internet.
//!
//! Two things have to be true in the same place for `https://<hostname>` to
//! work: something the public internet can reach must answer on it, and that
//! something must hold a certificate for that exact name. This command makes
//! them true in that order, and the order is the whole design.
//!
//! **The edge is configured first, then the record moves, then the
//! certificate arrives.** Caddy cannot obtain a certificate for a name that
//! does not yet resolve to it: both HTTP-01 and TLS-ALPN-01 are challenges
//! Let's Encrypt delivers *to the edge, through DNS*. So the site block has to
//! exist before the record moves — otherwise the first request to arrive finds
//! a proxy that has never heard of the name — and the certificate can only be
//! issued after it. That makes the third step load-bearing rather than
//! cosmetic: the hostname is polled until it answers over TLS, and the elapsed
//! time is reported, because between the record and the certificate there is a
//! real window in which the name resolves to an edge that cannot yet complete
//! a handshake. Success is never reported on anything less than a completed
//! TLS request. `stado web remove` runs the reverse: the record goes first, so
//! nothing resolves to a hostname the edge is about to stop terminating.
//!
//! **Publication is verified from outside, and Vercel is the thing it looks
//! for.** `curl -sI https://preferences.wisent.com/` answers 200 with
//! `server: Vercel` and an `x-vercel-id` header today, and the fleet is
//! already serving those bytes — Vercel contributes exactly one thing, a
//! certificate for a `wisent.com` name. So a 200 alone proves nothing: it is
//! the same 200 the hostname returned before this command ran. The check is a
//! 2xx **and** the absence of `x-vercel-id`, and a hostname still answering
//! from Vercel is reported as unpublished with the `server` and `x-vercel-id`
//! values that were actually observed. Nothing here removes a Vercel project;
//! a hostname stops being served by Vercel when its record stops pointing
//! there, and that record is this command's last step.
//!
//! **A hostname on the `cloudflare` edge is carried by the Cloudflare
//! tunnel.** [`crate::cli::cloudflare`] speaks tunnel routing, and
//! `cloudflare.rs` publishes such a hostname through it with the credentials
//! playing the Cloudflare roles in the owner vault: no router mapping or public
//! address is needed, and nothing falls back to the Stado edge, which would
//! publish the name from an edge the declaration did not choose.
//!
//! One step per file, in the order the command takes them: `publish/` holds
//! the edge, the record and the proof, `publish/verification.rs` the proof,
//! `planning.rs` the DNS half read without writing, `retract.rs` the reverse
//! for `stado web remove`, and `cloudflare.rs` the tunnel edge. The
//! constants below are the vocabulary all of them share.

use super::CmdError;

mod cloudflare;
mod planning;
mod publish;
mod retract;

pub(crate) use retract::retract;

use publish::publish;

/// The record every `stado`-edge hostname gets: an A record at the edge's own
/// public address.
const RECORD_TYPE: &str = "A";

/// The Skarbiec item holding the registrar's `api_user`, `api_key`, `username`
/// and `client_ip`, as the edge declaration names it. A product's hostname and
/// an operator's hand-typed `stado dns set` take one path through the
/// registrar; the item is declared once, beside the edge whose address the
/// records carry, and never built in.
fn registrar_credential(edge: &crate::config::WebApiEdge) -> Result<&str, CmdError> {
    edge.registrar_credential().ok_or_else(|| {
        CmdError::declaration(
            "web_api.edge declares no registrar_credential, so no Skarbiec item can write this \
             hostname's record; name the item holding the registrar's api_user, api_key, username \
             and client_ip with `stado web edge declare --target <host> --address <ipv4> \
             --contact <mail> --registrar-credential <item>`"
                .to_string(),
        )
    })
}

/// The header a Vercel edge stamps on every response it serves. Its presence
/// is the one unambiguous proof that a hostname has not moved to the fleet.
const VERCEL_HEADER: &str = "x-vercel-id";

pub(crate) async fn route(name: &str, check: bool, json: bool) -> Result<(), CmdError> {
    let declared = super::product(name)?;
    match declared.edge() {
        "stado" => publish(name, declared, check, json).await,
        "cloudflare" => cloudflare::publish(declared, check, json).await,
        other => Err(CmdError::declaration(format!(
            "web product {name} declares edge {other:?}, and no publication path implements it"
        ))),
    }
}
