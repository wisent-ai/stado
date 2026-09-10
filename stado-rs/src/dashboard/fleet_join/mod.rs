//! Invite-authenticated enrollment: the machine being added adds itself.
//!
//! Three routes, all reachable by a machine that holds nothing but a one-line
//! invite code and no Stado credentials at all:
//!
//! * `GET  /join.sh`               — the bootstrap script, unauthenticated
//! * `GET  /api/fleet/invite/key`  — the fleet's PUBLIC key for this invite
//! * `POST /api/fleet/join`        — record the machine's pending request
//!
//! Key direction is fixed and one-way: the fleet dials the machine, so the
//! machine receives a public key for its `authorized_keys` and the private
//! half never leaves the operator's vault. Nothing here mints, reads or
//! forwards private key material.
//!
//! Authorization on the two API routes is the invite token and nothing else.
//! Operator credentials are never consulted and never accepted: a bearer that
//! is an operator session, or a loopback caller that the operator routes trust
//! implicitly, is refused exactly like an unknown code. These routes never
//! write the registry — approval does that, from the operator's side, through
//! the probing `fleet enroll` path.
//!
//! The invite lifecycle itself lives in [`crate::cli::fleet::invite`] and is
//! not reimplemented here: this module parses the token, reads the object
//! once (keeping the version it needs for a compare-and-swap spend), and asks
//! that module what the invite's status actually is.
//!
//! Refusals are uniform. Unknown, spent, revoked, expired, malformed, and
//! rate-limited all produce the same status, the same body, and the same
//! floor on elapsed time (`REFUSAL_FLOOR`), so a caller cannot use the
//! endpoint to learn which of those states a code is in, nor to enumerate
//! codes by timing.
//!
//! Rate limiting here is deliberately NOT `crate::rate_limit::RateLimiter`,
//! the shared limiter the dashboard exposes on `/api/rate-limit/consume`, and
//! it must not be "unified" with it later. That limiter (a) authenticates the
//! caller as a configured `RateLimitClient` from Skarbiec, which a machine
//! holding only an invite code cannot be, and (b) persists its window state to
//! the object store on every allowed consume — so wiring an unauthenticated
//! route into it converts a request flood into one object-store write per
//! request. That is the cost this limiter exists to bound, not a way to bound
//! it. The window below is process-local, checked before any store or vault
//! read, and costs one mutex.

use crate::cli::fleet::invite;

use super::Request;

mod redeem;
mod refusals;
mod routes;
mod window;

pub(super) use routes::{invite_key, join, join_script};

/// The joining machine's report is a handful of short strings; anything
/// larger is a mistake or an attempt to make the dashboard allocate.
pub(super) const MAX_REQUEST_BYTES: usize = 4096;

/// The bootstrap script, embedded verbatim from the repository's
/// `deploy/join/` fragments by `build.rs`. Empty means the tree had none.
const JOIN_SCRIPT: &str = include_str!(concat!(env!("OUT_DIR"), "/join.sh"));

/// The embedded script itself, for the one caller that must compare against
/// exactly what this build serves rather than against a second copy of it:
/// `stado fleet ingress up` fetches `/join.sh` back through the tunnel it just
/// opened and checks the bytes. Reading `deploy/join/` off disk there would
/// prove the tunnel reaches *a* listener; reading this constant proves it
/// reaches one serving this binary's script.
pub(super) fn join_script_source() -> &'static str {
    JOIN_SCRIPT
}

/// Store prefix of the join requests these routes file.
const REQUESTS_PREFIX: &str = "enrollments/";
const STATUS_PENDING: &str = "pending";

/// Longest accepted value for any single reported string field.
const MAX_FIELD_BYTES: usize = 255;

fn bearer(request: &Request) -> Option<&str> {
    request
        .header("authorization")
        .map(str::trim)
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Token id and secret, owned, when the presented bearer has the right shape.
/// Shape failures are refusals like any other; the id is kept because the
/// limiter charges the request before anything else looks at it.
fn presented(request: &Request) -> Option<(String, String)> {
    let raw = bearer(request)?;
    invite::parse_token(raw)
        .ok()
        .map(|(id, secret)| (id.to_string(), secret.to_string()))
}

fn request_path(hostname: &str) -> String {
    format!("{REQUESTS_PREFIX}{hostname}.json")
}
