use super::*;

// ---------------------------------------------------------------------------
// registry.json — service directory and placement profiles
// ---------------------------------------------------------------------------

/// Which host publishes the service directory, and with which binary.
///
/// One authority per fleet: a directory written from two boxes is two
/// directories, and the loser silently serves endpoints nobody is listening
/// on any more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryAuthority {
    /// Registry target name of the publishing host.
    pub target: String,
    /// Absolute path of the `stado` binary that publishes from it.
    pub command: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Where one host answers for a service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub url: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What one consumer is entitled to ask a service for. The directory is the
/// only place this is written down, so a consumer absent from the map is not
/// authorized rather than unrestricted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceConsumer {
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub capabilities: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Speak HTTP and take any answer as proof that something is serving, 401,
/// 404 and 503 included. Health is a different question from existence, and
/// the outage this machinery came from was an endpoint that answered nothing
/// at all.
pub const VERIFY_KIND_HTTP: &str = "http";
/// Open a TCP connection and close it, for an endpoint that speaks no HTTP.
/// It proves a listener is accepting on the address the declaration hands out
/// — which is the whole of what the directory promises for such a service,
/// and everything an HTTP GET would have lied about.
pub const VERIFY_KIND_TCP: &str = "tcp";
/// Probe from every host the directory hands a dial address to: `service
/// directory publish` writes `endpoints[<this host>]` into that host's
/// forward file, so every one of them has been handed something it will one
/// day call and can be held to it. Standby addresses are not in this set —
/// they live in [`Service::standby`], nothing is meant to answer on them
/// yet, and probing one manufactures an outage out of a declaration.
pub const VERIFY_FROM_ENDPOINT_HOLDERS: &str = "endpoint-holders";
/// Probe only where the service claims to serve. For an endpoint no other
/// host is expected to reach — a socket bound behind a local-only guard —
/// where probing from elsewhere manufactures `unreachable` for a service
/// working exactly as declared.
pub const VERIFY_FROM_ACTIVE_HOST: &str = "active-host";
/// Anything at all came back. The only `expect` this build implements, and
/// the reading `service verify` already had.
pub const VERIFY_EXPECT_ANY_RESPONSE: &str = "any-response";

/// The values this build implements. One list per field, read by both the
/// validator and the prober: two lists is how a descriptor becomes valid at
/// validation time and unimplemented at probe time, which is the exact class
/// of gap — a declaration nothing reads — that this field exists to close.
pub const VERIFY_KINDS: [&str; 2] = [VERIFY_KIND_HTTP, VERIFY_KIND_TCP];
/// Vantages this build implements. See [`VERIFY_KINDS`].
pub const VERIFY_FROMS: [&str; 2] = [VERIFY_FROM_ENDPOINT_HOLDERS, VERIFY_FROM_ACTIVE_HOST];
/// Verdicts this build implements. See [`VERIFY_KINDS`].
pub const VERIFY_EXPECTS: [&str; 1] = [VERIFY_EXPECT_ANY_RESPONSE];

fn default_verify_kind() -> String {
    VERIFY_KIND_HTTP.to_string()
}

fn default_verify_from() -> String {
    VERIFY_FROM_ENDPOINT_HOLDERS.to_string()
}

fn default_verify_expect() -> String {
    VERIFY_EXPECT_ANY_RESPONSE.to_string()
}

/// How one declaration is checked against the world, written down beside the
/// declaration itself.
///
/// `service verify` shipped with a single probe wired into it: HTTP GET, from
/// every host holding an endpoint. That is the right question for every entry
/// the directory holds today and the wrong one for the first entry that is not
/// an HTTP service — and the danger is not that such a checker declines, it is
/// that it answers. A Postgres socket understands nothing an HTTP client says
/// and would be called `unreachable` while serving; a service only its own
/// host may dial would be called `unreachable` from four hosts that were never
/// meant to reach it. Both are verdicts on evidence nobody gathered, and a
/// fleet that files those learns to ignore its own reports — the same ending
/// as the silence they replace.
///
/// So the method travels with the declaration: adding a kind of service to the
/// directory means saying how it is seen, in the object that says where it
/// runs. A value this build does not implement yields `unverified` and never a
/// verdict, and [`validate_verification`] raises it against the author while
/// the registry is validated rather than against an operator reading a sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyDescriptor {
    /// What to speak at the endpoint: [`VERIFY_KIND_HTTP`] or
    /// [`VERIFY_KIND_TCP`].
    #[serde(default = "default_verify_kind")]
    pub kind: String,
    /// Which vantage the probe runs from: [`VERIFY_FROM_ENDPOINT_HOLDERS`] or
    /// [`VERIFY_FROM_ACTIVE_HOST`].
    #[serde(default = "default_verify_from")]
    pub from: String,
    /// What counts as proof: [`VERIFY_EXPECT_ANY_RESPONSE`] today.
    #[serde(default = "default_verify_expect")]
    pub expect: String,
    /// Keys this build does not model, kept verbatim. [`Registry::extra`]
    /// exists for the same reason one level up: on 2026-08-04 the canonical
    /// document lost three top-level blocks to a writer that could not name
    /// them, and a descriptor is no safer — a newer publisher's
    /// `verify.timeout_seconds` must survive a rewrite from this checkout.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `Map<String, Value>` blocks the `Eq` derive because `serde_json` will not
/// promise reflexivity for floats. It holds here regardless:
/// `serde_json::Number::from_f64` rejects NaN, so no NaN can reach a parsed or
/// constructed descriptor. Spelling it out is what lets
/// `service_resolution::ServiceRoute` keep the `Eq` it already had while
/// carrying one of these.
impl Eq for VerifyDescriptor {}

impl Default for VerifyDescriptor {
    fn default() -> Self {
        Self {
            kind: default_verify_kind(),
            from: default_verify_from(),
            expect: default_verify_expect(),
            extra: Map::new(),
        }
    }
}

/// Every problem in one verification descriptor, located for its author.
///
/// A descriptor is a promise that something goes and looks. A `kind` no build
/// implements is a promise nobody keeps, and the prober can only report that
/// one host at a time as `unverified`, inside a sweep somebody has to be
/// reading — the same shape as the twelve-day silence the sweep was written to
/// end. Raising it where the registry is validated puts the complaint in front
/// of the person typing the word.
///
/// Every problem rather than the first: a descriptor with a wrong `kind` and a
/// wrong `from` must not cost two trips through a document that needs a
/// signing key to rewrite.
///
/// NOT called from [`validate_registry`], which checks the raw registry-v2
/// contract and has never modelled the service directory. Directory entries
/// are validated in `service_resolution::validate_registry_contract`, and that
/// is where this is wired in.
pub fn validate_verification(location: &str, descriptor: &VerifyDescriptor) -> Vec<String> {
    [
        ("kind", descriptor.kind.as_str(), VERIFY_KINDS.as_slice()),
        ("from", descriptor.from.as_str(), VERIFY_FROMS.as_slice()),
        (
            "expect",
            descriptor.expect.as_str(),
            VERIFY_EXPECTS.as_slice(),
        ),
    ]
    .into_iter()
    .filter_map(|(field, value, known)| verify_problem(location, field, value, known))
    .collect()
}

/// One field's complaint, naming the word the author wrote and the words this
/// build answers to. The offending value is quoted back because a descriptor
/// is usually wrong by a character.
fn verify_problem(location: &str, field: &str, value: &str, known: &[&str]) -> Option<String> {
    if known.contains(&value) {
        return None;
    }
    Some(format!(
        "{location}.verify.{field}: unknown value '{value}'; this build implements {}",
        py_list_repr(known)
    ))
}
