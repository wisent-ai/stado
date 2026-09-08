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
//! **A zone at Cloudflare takes the other edge, and cannot be exercised
//! today.** [`crate::cli::cloudflare`] already speaks tunnel routing, and the
//! credential it needs does not exist in Skarbiec — so that arm refuses with
//! that fact rather than half-working or quietly falling back to the Stado
//! edge, which would publish the name from an edge the operator did not
//! choose.
//!
//! One step per file, in the order the command takes them: `publish/` holds
//! the edge, the record and the proof, `publish/verification.rs` the wait,
//! `planning.rs` the DNS half read without writing, `retract.rs` the reverse
//! for `stado web remove`, and `cloudflare.rs` the other edge's refusal. The
//! constants below are the vocabulary all of them share.

use std::time::Duration;

use super::CmdError;

mod cloudflare;
mod planning;
mod publish;
mod retract;

pub(crate) use retract::retract;

use cloudflare::cloudflare_unavailable;
use publish::publish;

/// The record every `stado`-edge hostname gets: an A record at the edge's own
/// public address.
const RECORD_TYPE: &str = "A";

/// Half an hour. Long enough that the registrar is not asked about a
/// production name on every request, short enough that moving the edge is a
/// half-hour cutover rather than a day.
const RECORD_TTL: &str = "1800";

/// The Skarbiec item holding the registrar's `api_user`, `api_key`, `username`
/// and `client_ip`. The same default `stado dns` uses, because a product's
/// hostname and an operator's hand-typed record must take one path through the
/// registrar.
const REGISTRAR_CREDENTIAL: &str = "namecheap_auto";

/// The header a Vercel edge stamps on every response it serves. Its presence
/// is the one unambiguous proof that a hostname has not moved to the fleet.
const VERCEL_HEADER: &str = "x-vercel-id";

/// How long the hostname is given to answer over TLS from the edge.
///
/// Three things happen inside this window, in order: the previous record's
/// TTL expires, the new record propagates, and Let's Encrypt answers a
/// challenge it delivers to the edge over the record that just moved. The
/// record this command writes carries [`RECORD_TTL`], but the one that governs
/// the wait is whatever TTL the *previous* record carried, and a first-time
/// issuance adds its own. Five minutes covers all three; beyond that something
/// is wrong and saying so beats waiting.
const VERIFY_BUDGET: Duration = Duration::from_secs(300);

/// Gap between verification attempts.
const VERIFY_INTERVAL: Duration = Duration::from_secs(5);

/// Per-request ceiling for one verification attempt.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) async fn route(name: &str, check: bool, json: bool) -> Result<(), CmdError> {
    let declared = super::product(name)?;
    match declared.edge() {
        "stado" => publish(name, declared, check, json).await,
        "cloudflare" => Err(CmdError::click(cloudflare_unavailable(declared.hostname()))),
        other => Err(CmdError::click(format!(
            "web product {name} declares edge {other:?}, and no publication path implements it"
        ))),
    }
}
