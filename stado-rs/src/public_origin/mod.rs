//! `public_origins`: the fleet's declaration of every hostname the public
//! internet is allowed to reach a Stado surface through.
//!
//! A public origin was not a declaration before this module. The one durable
//! release origin — `https://stado.wisent.com/api/release/object` promises
//! bearer-free reads under `stado://releases/` — is served by an edge that
//! fetches its bytes from somewhere else, and that somewhere else lived in one
//! untyped deployment environment variable. On 2026-09-07 its value was
//! `https://charless-mac-mini.tail6443b3.ts.net`, a MagicDNS name that answers
//! only inside this tailnet: `ts.net`'s own authoritative nameserver returns
//! NXDOMAIN for it, so every public read answered HTTP 503 with
//! `originDiagnosis.state = dns_unresolved`, and `version-check` refused every
//! pull request with `error_code=infra_down`. Nothing in the product had
//! declared that origin, nothing had refused it, and nothing could report it.
//! The same shape was measured the same day for `https://brama.wisent.com`,
//! which answers 502 `DNS_HOSTNAME_NOT_FOUND` at its edge. Two instances of one
//! defect: a published product hostname that no public resolver can answer.
//!
//! So the origin becomes a row in the canonical registry, and three things
//! confront it with the world:
//!
//! - [`validate_registry_contract`] is the offline half, run by every reader
//!   and by every write. It judges shape, and it judges the two things a
//!   document can be wrong about on its own: a hostname that is one of a
//!   target's declared control-route destinations, which `/docs/channels`
//!   forbids ("Release clients do not choose a network provider or derive that
//!   origin from a host's control route"), and a tailnet name whose node label
//!   is not the target that is supposed to publish it.
//! - [`resolve`] is the network half, run when a declaration is WRITTEN and
//!   whenever it is reported. It asks a public resolver — deliberately not this
//!   machine's, which knows MagicDNS names no client outside the tailnet can
//!   resolve — and answers in the three words `/docs/channels` already
//!   specifies: `dns_unresolved`, `dns_resolved`, `dns_unavailable`.
//! - [`funnel`] is the convergence half: it reads the declared target's
//!   publication and makes it match the declaration, through the host channel
//!   and its owning typed command, never a hand-run tunnel verb.
//!
//! The key is TOP-LEVEL and unmodelled by [`crate::targets::Registry`], so it
//! round-trips through `Registry::extra` and a build that predates it preserves
//! it instead of deleting it on the next write — the 2026-08-04 accident that
//! lost `channels`, `enrollment` and `fleets`. Validating it here is what makes
//! an operator learn at the write.

use serde_json::Value;

pub mod funnel;
pub mod resolve;
mod validate;

pub use resolve::{resolve, Resolution, ResolutionState};
pub use validate::validate_registry_contract;

/// The top-level registry key holding every public-origin declaration.
pub const POLICY_KEY: &str = "public_origins";

/// The one publication method implemented today.
///
/// A public origin needs a reachable endpoint and a certificate for its own
/// name. No fleet host holds a public address, and Tailscale Funnel routes by
/// SNI and holds a certificate for no name outside `*.ts.net`, so the fleet's
/// one free public entrance publishes a `*.ts.net` name and nothing else. The
/// field is a closed vocabulary rather than a free string because a
/// publication nothing can converge is a declaration with no reality check.
pub const TAILSCALE_FUNNEL: &str = "tailscale-funnel";

/// Every publication method a declaration may name.
pub const PUBLICATIONS: &[&str] = &[TAILSCALE_FUNNEL];

/// The largest number of paths one origin may publish. A publication is a set
/// of handler rules on a host; an unbounded list would be an unbounded write.
pub const MAX_PATHS: usize = 32;

/// One declared public origin.
///
/// `hostname` is a bare DNS name: no scheme, no port, no path. The scheme is
/// not a declaration — a public origin is HTTPS or it is not public — and a
/// port would be a second answer to a question the publication already owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicOrigin {
    /// Lowercase identity, unique in the document.
    pub name: String,
    /// The public DNS name a client resolves.
    pub hostname: String,
    /// The registry target whose publication serves it.
    pub target: String,
    /// How that target publishes it; one of [`PUBLICATIONS`].
    pub publication: String,
    /// The loopback origin on that target the publication forwards to.
    pub upstream: String,
    /// The exact absolute paths this origin publishes.
    pub paths: Vec<String>,
}

impl PublicOrigin {
    /// The origin as a client writes it: HTTPS and the declared name.
    pub fn origin(&self) -> String {
        format!("https://{}", self.hostname)
    }

    /// The upstream URL one declared path forwards to.
    ///
    /// Tailscale's own handler table records a per-path proxy target, so the
    /// declared path is appended to the declared loopback origin rather than
    /// replacing it: `/api/release/object` on `http://127.0.0.1:8765` is
    /// `http://127.0.0.1:8765/api/release/object`, which is what the live node
    /// already publishes and what a reconcile has to compare against.
    pub fn upstream_for(&self, path: &str) -> String {
        format!("{}{path}", self.upstream)
    }
}

/// Every declaration in one registry document, in declaration order.
///
/// An absent key is an empty list, never an error: a fleet that publishes
/// nothing publicly is a fleet with no rows here. A malformed key cannot reach
/// this function, because [`validate_registry_contract`] refuses the document
/// before any reader parses it — but a reader holding a last-known-good copy
/// written by an older build can, so a row that does not parse is skipped
/// rather than panicked on, and the validator is what reports it.
pub fn declarations(document: &Value) -> Vec<PublicOrigin> {
    let Some(rows) = document.get(POLICY_KEY).and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter().filter_map(parse_row).collect()
}

/// One declaration by name, or `None` when nothing declares it.
pub fn declaration(document: &Value, name: &str) -> Option<PublicOrigin> {
    declarations(document)
        .into_iter()
        .find(|origin| origin.name == name)
}

fn parse_row(row: &Value) -> Option<PublicOrigin> {
    let row = row.as_object()?;
    let text = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
    let paths = row
        .get("paths")?
        .as_array()?
        .iter()
        .map(|path| path.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    Some(PublicOrigin {
        name: text("name")?,
        hostname: text("hostname")?,
        target: text("target")?,
        publication: text("publication")?,
        upstream: text("upstream")?,
        paths,
    })
}

/// The declaration as the registry document carries it.
pub fn to_row(origin: &PublicOrigin) -> Value {
    serde_json::json!({
        "name": origin.name,
        "hostname": origin.hostname,
        "target": origin.target,
        "publication": origin.publication,
        "upstream": origin.upstream,
        "paths": origin.paths,
    })
}

/// The sentence a write is refused with when the declared name cannot be
/// resolved by a client outside this deployment.
///
/// It names the origin, the hostname and the repair, in that order, because an
/// operator reading it is deciding what to do next: the name is what they
/// typed, the hostname is what was judged, and "publish the name first" is the
/// only order that works — a public origin that does not resolve is not a
/// public origin, whatever is listening behind it.
pub fn unresolvable_refusal(name: &str, hostname: &str) -> String {
    format!(
        "refusing to declare public origin {name:?}: {hostname} has no public A or AAAA record, \
         so no public edge could fetch it; publish the name first, then declare it"
    )
}
