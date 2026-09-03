//! The Stado API listener: the authenticated object, release, machine,
//! service, host-health and enrollment HTTP surface. Port of
//! `stado/dashboard.py` (ThreadingHTTPServer) with the HTML operator
//! dashboard removed — the operator workspace is Stado Desktop, and this
//! listener serves no page.
//!

//! GET/PUT/DELETE /api/object?uri=stado://... - product object data plane
//! GET /api/object/list?namespace=...&prefix=... - product object listing
//! GET /api/object/stat?uri=stado://... - product object metadata
//! POST /api/object/compose - atomically publish verified object chunks
//! PUT /api/host-health?host=... - route-scoped authenticated host beacon publication
//! GET /api/release/object?uri=stado://releases/... - public software release download
//! POST /api/machine/submit - submit a canonical machine request
//! GET /api/machine/status?job_id=... - read canonical machine status
//! POST /api/machine/cancel?job_id=... - durably cancel a machine job
//! GET /api/service/status?name=... - read one managed service's beacon status
//! POST /api/service/restart?name=... - restart one managed service on every declared host
//! POST /api/rate-limit/consume - authenticated shared atomic rate-limit consume
//! POST /api/integration/enterprise/<action> - authenticated read-only fleet projection
//! GET /api/fleet/invite/key - invite-token-authenticated public channel key
//! POST /api/fleet/join - invite-token-authenticated pending enrollment request
//! GET /join.sh           - machine-side enrollment bootstrap script (public)
//! GET /healthz           - liveness (before auth, after the Host guard)
//! GET /livez             - Cloud Run liveness alias
//!
//! `--enrollment-only` narrows this listener to exactly three of the routes
//! above — `GET /join.sh`, `GET /api/fleet/invite/key`,
//! `POST /api/fleet/join` — and answers 404 to every other path and method
//! before authorization, the store or the vault is touched. That mode exists
//! so the enrollment routes can be published through a tunnel without
//! publishing the object, machine or service planes with them. See
//! `ENROLLMENT_ROUTES`.
//!
//! The application plane was extracted into the private `wisent-backend`
//! service; Stado keeps only the generic object plane. The product
//! integrations (Stripe, Resend, SendGrid, GitHub, HuggingFace, captcha
//! proxies) were extracted into the private `wisent-integrations` service.
//!
//! DEVIATIONS from Python (deliberate):
//! - Hand-rolled minimal HTTP/1.1 on `tokio::net::TcpListener`, one task per
//!   accepted connection (the ThreadingHTTPServer equivalent); the port
//!   spec forbids adding a web-framework dependency. Python's implicit
//!   `Server:`/`Date:` response headers and its error-page HTML bodies are
//!   not reproduced (status codes and JSON bodies match).

mod fleet_join;
mod integration;
mod registry_policy;

use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex as AsyncMutex;

use crate::config;
use crate::deploy::{host_channel, production_runner, service};
use crate::machine::{MachineError, MachineFacade, SCHEMA_VERSION as MACHINE_SCHEMA_VERSION};
use crate::models::isoformat_utc;
use crate::object_store::{ObjectRef, OBJECT_API_CHUNK_BYTES};
use crate::queue::submit::json_dumps_sorted_compact;
use crate::queue::{JobStorage, StorageError};
use crate::rate_limit::{self, ConsumeRequest, RateLimitError, RateLimiter};
use crate::targets;

/// Dashboard serve failure.
#[derive(Debug, thiserror::Error)]
pub enum DashboardError {
    /// Storage failures from the data-plane routes.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Listener/socket failures.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Port validation and other serve failures.
    #[error("{0}")]
    Other(String),
}

/// One authorization boundary this listener gates its routes on.
///
/// An enum rather than seven named booleans because the recovery path needs
/// to name a boundary as a value: claim its cooldown, run exactly its
/// verifier, record exactly its verdict. Seven fields could only be reached
/// by seven copies of that sequence, which is how the startup block came to
/// hold seven near-identical macro expansions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Object,
    Release,
    Machine,
    Service,
    RateLimitVerifier,
    RateLimitState,
    Integration,
    /// The registry-policy and cleanup routes the desktop app calls.
    ///
    /// Added on 2026-09-02, when all four of those routes answered 404 and
    /// the Swift client that calls them had been written against them for
    /// some time. Its verifier is ready even when nothing is declared: an
    /// undeclared boundary refuses every request with `401`, which is what
    /// "nobody has been granted this" means, and reporting it as unavailable
    /// would send an operator looking for a broken vault.
    Registry,
}

impl Boundary {
    /// Every boundary, in the deterministic order startup validates them.
    /// Also the order the served documents list them in, so an operator
    /// comparing a health document with a startup log reads one sequence.
    const ALL: [Boundary; 8] = [
        Boundary::Object,
        Boundary::Release,
        Boundary::Machine,
        Boundary::Service,
        Boundary::RateLimitVerifier,
        Boundary::RateLimitState,
        Boundary::Integration,
        Boundary::Registry,
    ];

    /// Which route requires this boundary. Every entry here was verified by
    /// reading the `boundaries_available` call sites, and the list is in the
    /// type so the next reader checks it instead of re-deriving it:
    ///
    /// - `Object` — `/api/object` PUT, `/api/object`, `/api/object/list`,
    ///   `/api/object/stat`, and the two POST object routes.
    /// - `Release` — the same object routes when the coordinate resolves to a
    ///   release policy, because `authorize_release` reads that verifier's
    ///   material. It required NO route until 2026-08-31: enumerated,
    ///   labelled, described, validated once at startup, reported in
    ///   `/healthz`, and consulted nowhere — so it read `false` until a
    ///   restart and no request could reopen it, because
    ///   `boundaries_available` revalidates only what a request requires.
    /// - `Machine` — `/api/machine/status`, `/api/machine/submit`,
    ///   `/api/machine/cancel`.
    /// - `Service` — `/api/service/status`, `/api/service/restart`.
    /// - `RateLimitVerifier` and `RateLimitState` — `/api/rate-limit/consume`.
    /// - `Integration` — the integration route group.
    ///
    /// A boundary that answers this question with "nothing" must not be
    /// reported: an operator reading `/healthz` has to be able to conclude
    /// something true from every field in it.
    fn required_by(self) -> &'static str {
        match self {
            Boundary::Object => "/api/object, /api/object/list, /api/object/stat",
            Boundary::Release => "the object routes for a release coordinate",
            Boundary::Machine => "/api/machine/status, /api/machine/submit, /api/machine/cancel",
            Boundary::Service => "/api/service/status, /api/service/restart",
            Boundary::RateLimitVerifier | Boundary::RateLimitState => "/api/rate-limit/consume",
            Boundary::Integration => "the integration routes",
            Boundary::Registry => {
                "/api/registry.json, /api/registry/policy, /api/cleanup.json, /api/cleanup/run"
            }
        }
    }

    /// The key this boundary carries in `/healthz`.
    /// Unchanged from the flat booleans `/healthz` has always served.
    fn key(self) -> &'static str {
        match self {
            Boundary::Object => "object",
            Boundary::Release => "release",
            Boundary::Machine => "machine",
            Boundary::Service => "service",
            Boundary::RateLimitVerifier => "rate_limit_verifier",
            Boundary::RateLimitState => "rate_limit_state",
            Boundary::Integration => "integration",
            Boundary::Registry => "registry",
        }
    }

    /// The name the dashboard logs use, and the incident vocabulary with it.
    fn label(self) -> &'static str {
        match self {
            Boundary::Object => "object authorization",
            Boundary::Release => "release publication",
            Boundary::Machine => "machine authorization",
            Boundary::Service => "service authorization",
            Boundary::RateLimitVerifier => "rate-limit authorization",
            Boundary::RateLimitState => "rate-limit state",
            Boundary::Integration => "integration authorization",
            Boundary::Registry => "registry authorization",
        }
    }
}

/// One boundary's live verdict.
///
/// This used to be a bare `bool` decided once at startup and never revisited,
/// so a single slow or reset vault read shut a boundary until somebody
/// restarted the unit — and `object` shutting answers `503 object
/// authorization unavailable` to the whole fleet. Recovery is a property of
/// this state now: `attempted_at` is the cooldown anchor an inline
/// revalidation claims before it runs.
#[derive(Clone, Default)]
struct BoundaryVerdict {
    ready: bool,
    /// The monotonic clock of the last validation attempt. Not a wall clock:
    /// a clock step must not be able to skip the cooldown or stretch it past
    /// the next request.
    attempted_at: Option<Instant>,
    /// The validator's own sentence for the last failure, or `None` while the
    /// boundary is open.
    ///
    /// Without this, a closed boundary is one bit and an operator cannot tell
    /// the two answers apart that need opposite responses: `validation did not
    /// settle within N seconds` is arithmetic against the item budget, while
    /// `item set mismatch` or `missing or empty` is a credential answer, and a
    /// credential answer is not fixed by restarting the process. On
    /// 2026-09-03 that distinction was unreachable for a live closed boundary:
    /// `/healthz` publishes booleans by design, the process holding the
    /// verdict logged no boundary line, and the doctor's remedy named
    /// `stado service logs com.wisent.always-on.stado-object-api`, which on
    /// that host answers `no unit file ... in the daemon or agent
    /// directories`. A remedy naming an unreadable artefact is worse than
    /// none.
    last_error: Option<String>,
    /// When that verdict was reached, in wall-clock terms, for the operator
    /// document. `attempted_at` above is monotonic and deliberately so — it
    /// anchors the cooldown and must survive a clock step — but a monotonic
    /// instant means nothing to a reader comparing this against a log.
    checked_at: Option<String>,
}

#[derive(Clone, Default)]
struct BoundaryAvailability {
    verdicts: [BoundaryVerdict; 8],
}

impl BoundaryAvailability {
    fn verdict(&self, boundary: Boundary) -> &BoundaryVerdict {
        &self.verdicts[boundary as usize]
    }

    fn verdict_mut(&mut self, boundary: Boundary) -> &mut BoundaryVerdict {
        &mut self.verdicts[boundary as usize]
    }

    fn ready(&self, boundary: Boundary) -> bool {
        self.verdict(boundary).ready
    }

    /// The flat booleans `/healthz` has always published. That route answers
    /// before authorization, so it stays booleans: a `last_error` names vault
    /// items, grants and endpoints, and an unauthenticated liveness probe has
    /// no business reading those.
    fn ready_json(&self) -> Value {
        Value::Object(
            Boundary::ALL
                .iter()
                .map(|boundary| (boundary.key().to_string(), json!(self.ready(*boundary))))
                .collect(),
        )
    }

    /// Every boundary with its verdict AND the validator's own sentence, for
    /// `/api/state.json`.
    ///
    /// Separate from [`Self::ready_json`] on purpose: `/healthz` is the
    /// unauthenticated liveness probe and stays booleans, this is the
    /// operator's read. The sentence is the verifier's own words about what
    /// refused — a reason and its subject — never the material itself, which
    /// the verifiers do not put in their error text.
    fn state_json(&self) -> Value {
        Value::Object(
            Boundary::ALL
                .iter()
                .map(|boundary| {
                    let verdict = self.verdict(*boundary);
                    (
                        boundary.key().to_string(),
                        json!({
                            "ready": verdict.ready,
                            "last_error": verdict.last_error,
                            "checked_at": verdict.checked_at,
                            "required_by": boundary.required_by(),
                            "label": boundary.label(),
                        }),
                    )
                })
                .collect(),
        )
    }

    fn all_ready(&self) -> bool {
        Boundary::ALL.iter().all(|boundary| self.ready(*boundary))
    }
}

/// What a request is allowed to do about a boundary it found closed.
enum Recheck {
    /// Already open; proceed.
    Ready,
    /// Closed, and this request owns the one revalidation attempt.
    Claimed,
    /// Closed, and an attempt inside the cooldown already answered for it.
    CoolingDown,
}

/// Path of the live boundary-budget override, relative to `$HOME`.
///
/// Owner-controlled state beside `skarbiec.vault.json` and the token files this
/// unit already reads out of `$HOME/.stado`. Its absence is the normal state.
pub const BOUNDARY_TIMEOUT_OVERRIDE_PATH: &str = ".stado/dashboard-boundary-timeout-seconds";

/// The override's current value, or nothing.
///
/// Read on every validation attempt on purpose: a budget that can only be
/// changed by restarting the process is not an override for a stalled process.
/// A missing file, an unreadable one, a non-numeric body and a zero all read as
/// "no override", so a typo cannot disable the boundary by setting the budget
/// to nothing.
fn file_override_seconds() -> Option<u64> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join(BOUNDARY_TIMEOUT_OVERRIDE_PATH);
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
}

/// How long one boundary validation attempt may run, at startup and on an
/// inline recheck alike.
///
/// Each boundary reads every item its policy names, and each read is a gpg
/// decryption in the broker. Seventeen object namespaces against a real vault
/// with a cold gpg-agent exceeded the previous 15s and the object API then
/// answered 503 to the entire fleet until someone restarted it -- a cold
/// agent is a slow start, not a broken grant.
fn boundary_timeout(boundary: Boundary) -> Duration {
    // The override a stalled unit can actually be given, read at validation
    // time from a file rather than from the environment.
    //
    // `WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS` below is honoured for a process
    // that was launched with it, and it stays. What it cannot do is help in the
    // situation it exists for: a boundary stalling in a RUNNING process. Rust
    // reads env at exec, so the only way to apply it was to restart the very
    // service whose stall was the problem — and on 2026-08-31 that service was
    // the object store a release was publishing through, and its boundaries
    // then recovered by themselves while a restart would have bought a fresh
    // cold gpg-agent and destroyed the evidence. An escape hatch that requires
    // restarting the thing it is escaping is decorative.
    //
    // A file is re-read on every attempt, so an operator raises the budget with
    // one `echo` and lowers it by deleting the file, with nothing cycled. It is
    // owner-controlled state beside the vault and the token files this unit
    // already reads from `$HOME/.stado`.
    if let Some(configured) = file_override_seconds() {
        return Duration::from_secs(configured);
    }
    if let Some(configured) = std::env::var("WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
    {
        return Duration::from_secs(configured);
    }
    // Per mapped item, not flat. Each verifier boundary reads its grant and
    // then ONE vault field per mapped item, strictly serially, because a
    // Skarbiec request decrypts and rewrites shared state and fanning out
    // caused resets (`skarbiec::validate::object`). So the work is linear in
    // the number of declarations, and a fixed 90 seconds is a budget that
    // stops being true as the fleet grows.
    //
    // On 2026-08-31 charless-mac-mini declared 17 object namespaces, 14
    // release publishers and 4 service deployers. Every boundary failed with
    // "validation did not settle within 90 seconds", every object route
    // answered 503, two `queue resume` attempts died on it, and no release
    // could publish — while the vault was up, listening, and answering. The
    // same lesson is already recorded one module over in
    // `doctor::object_auth_deadline`, which budgets this exact sweep per item;
    // this is that fix applied to the boundary the whole fleet reads through.
    let mapped = match boundary {
        Boundary::Object => {
            crate::config::object_api_namespaces().map_or(usize::MIN, |items| items.len())
        }
        Boundary::Release => {
            crate::config::release_api_publishers().map_or(usize::MIN, |items| items.len())
        }
        Boundary::Machine => {
            crate::config::machine_api_clients().map_or(usize::MIN, |items| items.len())
        }
        Boundary::Service => {
            crate::config::service_api_deployers().map_or(usize::MIN, |items| items.len())
        }
        Boundary::Registry => {
            crate::config::registry_api_clients().map_or(usize::MIN, |items| items.len())
        }
        // These read a fixed, small set of material rather than one item per
        // declaration, so they keep the flat allowance.
        Boundary::RateLimitVerifier | Boundary::RateLimitState | Boundary::Integration => {
            usize::MIN
        }
    };
    BOUNDARY_ITEM_ALLOWANCE + BOUNDARY_ITEM_ALLOWANCE * u32::try_from(mapped).unwrap_or(u32::MIN)
}

/// Allowance for one grant read plus one mapped item. The flat value this
/// replaces, kept as the unit, so a deployment that declares nothing sees the
/// budget it always had.
const BOUNDARY_ITEM_ALLOWANCE: Duration = Duration::from_secs(90);
/// boundary. Long enough that a fleet hammering a shut boundary produces one
/// vault sweep per cooldown rather than one per request, short enough that a
/// transient reset costs seconds of 503 instead of a privileged restart.
fn boundary_recheck_cooldown() -> Duration {
    Duration::from_secs(
        std::env::var("WC_DASHBOARD_BOUNDARY_RECHECK_SECONDS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(30),
    )
}

/// What one request does about the boundaries it touches.
///
/// **No boundary may be its own precondition for reopening.** That is the rule
/// this type exists to make expressible, and breaking it is how
/// [`Boundary::Release`] spent a night frozen at its boot verdict.
///
/// `boundaries_available` revalidates only what a request REQUIRES, and the
/// only routes that could revalidate `Release` were the release-coordinate
/// object routes — which [`requires_object_boundary`] excludes from the check
/// precisely because the key IS a release key. So the boundary was required by
/// nothing that could reach it: closed once, closed for the life of the
/// process, and no request, credential or amount of asking could reopen it.
/// Two reads on 2026-09-03 proved it — a successful stat and a rejected
/// object read, both against a release coordinate, `release` still `false`
/// after each.
///
/// Inverting the predicate does not fix that. It moves the deadlock from
/// silent to loud: every release-coordinate read on a process whose `release`
/// boundary is already shut would answer `503`, and that boundary is shut on
/// the resolver every host reaches its objects through. So asking and
/// enforcing are separated here instead.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BoundaryPlan {
    /// Boundaries this request may not proceed without. A closed one answers
    /// `503` and the request stops.
    enforced: Vec<Boundary>,
    /// Boundaries this request revalidates, whether or not it is gated by
    /// them. Always a superset of `enforced`: enforcing without asking is what
    /// froze `Release`, and asking without enforcing is what unfreezes it.
    revalidated: Vec<Boundary>,
}

impl BoundaryPlan {
    /// Gated by exactly what it revalidates: the ordinary case.
    fn gated(boundaries: &[Boundary]) -> Self {
        Self {
            enforced: boundaries.to_vec(),
            revalidated: boundaries.to_vec(),
        }
    }

    /// Revalidated and NOT gated.
    fn asked_only(boundaries: &[Boundary]) -> Self {
        Self {
            enforced: Vec::new(),
            revalidated: boundaries.to_vec(),
        }
    }

    fn none() -> Self {
        Self {
            enforced: Vec::new(),
            revalidated: Vec::new(),
        }
    }
}

/// Which boundaries one request enforces and which it revalidates.
///
/// One pure function, so the answer is the same for the router and for the
/// test that proves every boundary has a way back. `object` is the addressed
/// object's namespace and key for the object routes, and `None` elsewhere.
fn boundary_plan(path: &str, object: Option<(&str, &str)>) -> BoundaryPlan {
    if let Some((namespace, key)) = object {
        if crate::object_store::release_policy_key(namespace, key).is_some() {
            // Revalidation only, deliberately. `authorize_release` reads the
            // release verifier's material, so this boundary IS this request's
            // precondition in principle — but turning enforcement on is a
            // separate decision with a fleet-wide blast radius, and it cannot
            // be taken until a closed boundary can reopen at all. This is what
            // gives it that path. Enforcement stays off, so the traffic that
            // works today keeps working, and the field stops reporting a
            // verdict frozen at boot.
            return BoundaryPlan::asked_only(&[Boundary::Object, Boundary::Release]);
        }
        return BoundaryPlan::gated(&[Boundary::Object]);
    }
    match path {
        "/api/rate-limit/consume" => {
            BoundaryPlan::gated(&[Boundary::RateLimitVerifier, Boundary::RateLimitState])
        }
        "/api/machine/submit" | "/api/machine/cancel" | "/api/machine/status" => {
            BoundaryPlan::gated(&[Boundary::Machine])
        }
        "/api/service/restart" | "/api/service/status" => BoundaryPlan::gated(&[Boundary::Service]),
        path if path.starts_with("/api/integration/") => {
            BoundaryPlan::gated(&[Boundary::Integration])
        }
        _ => BoundaryPlan::none(),
    }
}

/// Release objects authorize against their exact product item at request time.
/// Only private product objects use the global object-verifier readiness gate.
///
/// A single release boundary cannot represent per-product readiness: making a
/// Stado request wait for every configured publisher coupled unrelated
/// products and returned 503 before the exact Stado item was even read.
///
/// That reasoning is about ENFORCEMENT and it still holds; it is expressed by
/// [`BoundaryPlan::asked_only`] in [`boundary_plan`] now, which keeps the gate
/// off for a release coordinate and revalidates the boundary anyway.
fn requires_object_boundary(namespace: &str, key: &str) -> bool {
    crate::object_store::release_policy_key(namespace, key).is_none()
}

/// The exact (method, path) pairs `--enrollment-only` serves.
///
/// This is an ALLOWLIST, and it must stay one. A denylist of the sensitive
/// surfaces (`/api/object`, `/api/machine/submit`, ...) goes stale the
/// moment somebody adds a route: the new route is published by
/// default, and the mistake is invisible until the wrong thing answers on a
/// public tunnel. With an allowlist a new route is unreachable in this mode
/// until it is named here on purpose, so forgetting fails closed.
const ENROLLMENT_ROUTES: [(&str, &str); 3] = [
    ("GET", "/join.sh"),
    ("GET", "/api/fleet/invite/key"),
    ("POST", "/api/fleet/join"),
];

/// The machine-side bootstrap script this build serves at `GET /join.sh`.
///
/// Published for `stado fleet ingress`, which verifies a newly opened tunnel
/// by fetching that route from the internet and comparing the bytes with what
/// the listener behind the tunnel would have served. Empty in a build whose
/// source tree had no `deploy/join.sh`, exactly as the route is.
pub fn join_script_source() -> &'static str {
    fleet_join::join_script_source()
}

/// The one body every refused request gets in `--enrollment-only` mode.
///
/// Uniform and mute on purpose: it names no route, no surface and no
/// credential, so a caller cannot learn from a refusal that an operator plane
/// exists elsewhere, nor tell "wrong method on a served path" from "path this
/// listener has never heard of".
const ENROLLMENT_REFUSAL: &[u8] = b"not found\n";

/// One representative request per route family [`boundary_plan`] knows, used
/// to prove every boundary has a way back.
const REOPENING_PROBES: &[(&str, Option<(&str, &str)>)] = &[
    ("/api/object", Some(("probierz", "queue/one.json"))),
    (
        "/api/object",
        Some(("releases", "stado/1.0.0/darwin-arm64/stado")),
    ),
    ("/api/object/list", Some(("sources", "stado/1.0.0"))),
    ("/api/rate-limit/consume", None),
    ("/api/machine/status", None),
    ("/api/service/status", None),
    ("/api/integration/anything", None),
];

/// Which boundaries no request can reopen.
///
/// **No boundary may be its own precondition for reopening.** A boundary
/// revalidates only when a request asks about it, so a boundary that appears
/// in no request's revalidation set is frozen at its boot verdict for the life
/// of the process — closed forever, or open forever, whichever it started as.
/// This answers that from the code rather than from a live host, which is the
/// only way it can be answered before it costs a night.
///
/// It has now been the same defect twice. `Boundary::Release` was enumerated,
/// labelled, described, validated at startup, published in `/healthz` and
/// required by NO route; the repair gave it the release-coordinate object
/// routes — and those are exactly the routes
/// [`requires_object_boundary`] excludes from the check, so the boundary
/// remained its own precondition and nobody noticed for another five hours.
/// A rule that is only ever checked by reading is a rule that gets re-broken
/// by the fix for it.
pub fn boundaries_without_a_reopening_route() -> Vec<&'static str> {
    Boundary::ALL
        .iter()
        .filter(|boundary| {
            !REOPENING_PROBES
                .iter()
                .any(|(path, object)| boundary_plan(path, *object).revalidated.contains(boundary))
        })
        .map(|boundary| boundary.key())
        .collect()
}

/// Whether one route family gates on a boundary it revalidates, for the test
/// that documents the release split as deliberate rather than accidental.
pub fn release_coordinate_boundary_split() -> (Vec<&'static str>, Vec<&'static str>) {
    let plan = boundary_plan(
        "/api/object",
        Some(("releases", "stado/1.0.0/darwin-arm64/stado")),
    );
    (
        plan.enforced
            .iter()
            .map(|boundary| boundary.key())
            .collect(),
        plan.revalidated
            .iter()
            .map(|boundary| boundary.key())
            .collect(),
    )
}

/// Whether `method`+`path` is one of the three enrollment pairs. The query
/// string is not part of the decision; the routes parse their own.
fn enrollment_route_allowed(method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or("");
    ENROLLMENT_ROUTES
        .iter()
        .any(|(allowed_method, allowed_path)| *allowed_method == method && *allowed_path == path)
}

const OBJECT_TOKEN_FRESH_FOR: Duration = Duration::from_secs(60);
const OBJECT_TOKEN_STALE_FOR: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
struct CachedObjectToken {
    value: Option<String>,
    loaded_at: Instant,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectComposeChunk {
    uri: String,
    size: usize,
    sha256: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectComposeRequest {
    uri: String,
    content_type: String,
    if_absent: bool,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    upload_id: String,
    size: usize,
    chunks: Vec<ObjectComposeChunk>,
}

#[derive(Clone)]
pub struct Dashboard {
    store: JobStorage,

    rate_limiter: RateLimiter,
    /// Every boundary's live verdict. Written by startup validation and by
    /// the inline recheck a request runs when it finds its boundary closed.
    boundaries: Arc<RwLock<BoundaryAvailability>>,
    /// Namespace bearer cache. Object traffic must not turn into one Skarbiec
    /// read per object request: that exhausted the broker's request capacity
    /// and made the whole object plane answer 503. One async lock also folds a
    /// cold-start burst into one vault read.
    object_tokens: Arc<AsyncMutex<BTreeMap<String, CachedObjectToken>>>,
    /// Release publisher bearers remain usable through a transient Skarbiec
    /// read failure after the release verifier has already proved them. Unlike
    /// object traffic, release traffic refreshes on every request so a token
    /// rotation takes effect immediately; this map is only the bounded
    /// last-known-good fallback.
    release_tokens: Arc<AsyncMutex<BTreeMap<String, CachedObjectToken>>>,
    /// Serve only [`ENROLLMENT_ROUTES`]; every other request is refused
    /// before authorization, before the store and before the vault.
    enrollment_only: bool,
}

impl Dashboard {
    /// Bind the listener to a storage facade.
    pub fn new(store: JobStorage) -> Self {
        Self {
            rate_limiter: RateLimiter::new(store.clone()),
            boundaries: Arc::new(RwLock::new(BoundaryAvailability::default())),
            object_tokens: Arc::new(AsyncMutex::new(BTreeMap::new())),
            release_tokens: Arc::new(AsyncMutex::new(BTreeMap::new())),
            store,
            enrollment_only: false,
        }
    }

    /// Serve only the enrollment routes (`stado dashboard
    /// --enrollment-only`), so this listener can be published through a
    /// tunnel without publishing anything else.
    pub fn with_enrollment_only(mut self, enrollment_only: bool) -> Self {
        self.enrollment_only = enrollment_only;
        self
    }
    async fn object_token(&self, namespace: &str, item: &str) -> Result<String, ()> {
        let mut tokens = self.object_tokens.lock().await;
        let now = Instant::now();
        if let Some(cached) = tokens.get(namespace) {
            if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_FRESH_FOR {
                return cached.value.clone().ok_or(());
            }
        }

        match crate::skarbiec::read_object_token(item, "token").await {
            Ok(Some(value)) if !value.is_empty() => {
                tokens.insert(
                    namespace.to_string(),
                    CachedObjectToken {
                        value: Some(value.clone()),
                        loaded_at: now,
                    },
                );
                Ok(value)
            }
            Ok(_) => {
                eprintln!("[dashboard] object verifier item unavailable for namespace {namespace}");
                tokens.insert(
                    namespace.to_string(),
                    CachedObjectToken {
                        value: None,
                        loaded_at: now,
                    },
                );
                Err(())
            }
            Err(error) => {
                if let Some(cached) = tokens.get(namespace) {
                    if let Some(value) = &cached.value {
                        if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_STALE_FOR {
                            eprintln!(
                                "[dashboard] object verifier refresh failed for namespace {namespace}; using the last token loaded {}s ago: {error}",
                                now.duration_since(cached.loaded_at).as_secs()
                            );
                            return Ok(value.clone());
                        }
                    }
                }
                eprintln!("[dashboard] object verifier failed for namespace {namespace}: {error}");
                tokens.insert(
                    namespace.to_string(),
                    CachedObjectToken {
                        value: None,
                        loaded_at: now,
                    },
                );
                Err(())
            }
        }
    }
    async fn release_token(&self, item: &str) -> Result<String, ()> {
        let mut tokens = self.release_tokens.lock().await;
        let now = Instant::now();
        if let Some(cached) = tokens.get(item) {
            if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_FRESH_FOR {
                return cached.value.clone().ok_or(());
            }
        }

        match crate::skarbiec::read_release_token(item, "token").await {
            Ok(Some(value)) if !value.is_empty() => {
                tokens.insert(
                    item.to_string(),
                    CachedObjectToken {
                        value: Some(value.clone()),
                        loaded_at: now,
                    },
                );
                Ok(value)
            }
            Ok(_) => {
                if let Some(cached) = tokens.get(item) {
                    if let Some(value) = &cached.value {
                        if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_STALE_FOR {
                            eprintln!(
                                "[dashboard] release verifier item unavailable for {item}; using \
                                 the last token loaded {}s ago",
                                now.duration_since(cached.loaded_at).as_secs()
                            );
                            return Ok(value.clone());
                        }
                    }
                }
                eprintln!("[dashboard] release verifier item unavailable: {item}");
                tokens.insert(
                    item.to_string(),
                    CachedObjectToken {
                        value: None,
                        loaded_at: now,
                    },
                );
                Err(())
            }
            Err(error) => {
                if let Some(cached) = tokens.get(item) {
                    if let Some(value) = &cached.value {
                        if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_STALE_FOR {
                            eprintln!(
                                "[dashboard] release verifier refresh failed for {item}; using the \
                                 last token loaded {}s ago: {error}",
                                now.duration_since(cached.loaded_at).as_secs()
                            );
                            return Ok(value.clone());
                        }
                    }
                }
                eprintln!("[dashboard] release verifier failed for {item}: {error}");
                tokens.insert(
                    item.to_string(),
                    CachedObjectToken {
                        value: None,
                        loaded_at: now,
                    },
                );
                Err(())
            }
        }
    }

    /// Run exactly one boundary's verifier once, bounded by
    /// [`boundary_timeout`], and flatten every failure shape — refusal,
    /// timeout, misconfiguration — into the one sentence an operator reads in
    /// the log and in `last_error`.
    async fn validate_boundary(&self, boundary: Boundary) -> Result<(), String> {
        let timeout = boundary_timeout(boundary);
        macro_rules! bounded {
            ($call:expr) => {
                match tokio::time::timeout(timeout, $call).await {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err(format!(
                        "validation did not settle within {} seconds, reading one vault field \
                         per mapped item serially; a vault that accepts connections without \
                         answering inside that budget looks identical to a missing grant here",
                        timeout.as_secs()
                    )),
                }
            };
        }
        match boundary {
            Boundary::Object => bounded!(crate::skarbiec::validate_object_verifier()),
            Boundary::Release => bounded!(crate::skarbiec::validate_release_verifier()),
            Boundary::Machine => bounded!(crate::skarbiec::validate_machine_verifier()),
            Boundary::Service => bounded!(crate::skarbiec::validate_service_verifier()),
            Boundary::RateLimitVerifier => bounded!(rate_limit::validate_verifier()),
            Boundary::RateLimitState => bounded!(self.rate_limiter.restore()),
            Boundary::Integration => bounded!(integration::validate_startup()),
            Boundary::Registry => bounded!(crate::skarbiec::validate_registry_verifier()),
        }
    }

    /// Record one validation outcome as this boundary's current verdict.
    fn record_boundary(&self, boundary: Boundary, outcome: Result<(), String>) {
        let verdict = BoundaryVerdict {
            ready: outcome.is_ok(),
            attempted_at: Some(Instant::now()),
            last_error: outcome.as_ref().err().cloned(),
            checked_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        *self
            .boundaries
            .write()
            .expect("dashboard boundary state lock")
            .verdict_mut(boundary) = verdict;
    }

    /// Whether `boundary` is open right now. Never touches the vault, so this
    /// is what every request on a healthy listener pays: one read lock.
    fn boundary_ready(&self, boundary: Boundary) -> bool {
        self.boundaries
            .read()
            .expect("dashboard boundary state lock")
            .ready(boundary)
    }

    /// Decide what this request may do about `boundary`, and — when it may
    /// revalidate — claim the attempt by stamping `attempted_at` before the
    /// vault is touched. Claiming under the write lock is what keeps a fleet
    /// hammering a shut boundary to one vault sweep per cooldown instead of
    /// one per request.
    fn claim_boundary_recheck(&self, boundary: Boundary) -> Recheck {
        if self.boundary_ready(boundary) {
            return Recheck::Ready;
        }
        let now = Instant::now();
        let cooldown = boundary_recheck_cooldown();
        let mut boundaries = self
            .boundaries
            .write()
            .expect("dashboard boundary state lock");
        let verdict = boundaries.verdict_mut(boundary);
        if verdict.ready {
            return Recheck::Ready;
        }
        if verdict
            .attempted_at
            .is_some_and(|attempted_at| now.duration_since(attempted_at) < cooldown)
        {
            return Recheck::CoolingDown;
        }
        verdict.attempted_at = Some(now);
        Recheck::Claimed
    }

    /// Ready-or-recover for one boundary: revalidate a closed boundary inline,
    /// at most once per cooldown, and answer whether the request may proceed.
    ///
    /// This is the recovery half of the startup sweep. Before it, a boundary
    /// closed by one slow or reset read stayed closed until a privileged unit
    /// restart — and for `com.wisent.always-on.stado-object-api` that restart
    /// is exactly the thing the fleet cannot do for itself.
    async fn recover_boundary(&self, boundary: Boundary) -> bool {
        match self.claim_boundary_recheck(boundary) {
            Recheck::Ready => return true,
            Recheck::CoolingDown => return false,
            Recheck::Claimed => {}
        }
        eprintln!(
            "[dashboard] {} boundary is closed; revalidating inline (required by {})",
            boundary.label(),
            boundary.required_by()
        );
        let outcome = self.validate_boundary(boundary).await;
        match &outcome {
            Ok(()) => eprintln!(
                "[dashboard] {} boundary recovered without a restart",
                boundary.label()
            ),
            Err(error) => eprintln!(
                "[dashboard] {} boundary revalidation failed: {error}",
                boundary.label()
            ),
        }
        let ready = outcome.is_ok();
        self.record_boundary(boundary, outcome);
        ready
    }

    /// Whether every boundary a route needs is open, revalidating at most one
    /// closed boundary. One per request on purpose: a single request must not
    /// be able to turn into a fan of vault sweeps, and the first closed
    /// boundary is the one the refusal already names.
    async fn boundaries_available(&self, required: &[Boundary]) -> bool {
        let mut attempted = false;
        for &boundary in required {
            if self.boundary_ready(boundary) {
                continue;
            }
            if attempted {
                return false;
            }
            attempted = true;
            if !self.recover_boundary(boundary).await {
                return false;
            }
        }
        true
    }

    /// Carry out one request's [`BoundaryPlan`]: revalidate what it asks
    /// about, then answer whether what it enforces is open.
    ///
    /// The two halves are deliberately different sets. `boundaries_available`
    /// fuses them — it revalidates exactly what it gates on — and that fusion
    /// is what let `Boundary::Release` become its own precondition. Here a
    /// request can ask about a boundary it is not gated by, which is the only
    /// way a boundary excluded from its own gate ever reopens.
    ///
    /// Still one vault sweep per request: a request must not turn into a fan
    /// of serial gpg decryptions, and the cooldown in
    /// [`Self::claim_boundary_recheck`] keeps a fleet hammering a shut
    /// boundary to one attempt per window.
    async fn satisfy_boundaries(&self, plan: &BoundaryPlan) -> bool {
        let mut attempted = false;
        for &boundary in &plan.revalidated {
            if self.boundary_ready(boundary) {
                continue;
            }
            if attempted {
                break;
            }
            attempted = true;
            self.recover_boundary(boundary).await;
        }
        plan.enforced
            .iter()
            .all(|&boundary| self.boundary_ready(boundary))
    }

    /// Start the boundary checks and serve HTTP on loopback. This server does
    /// not terminate TLS, so binding it to a non-loopback interface would
    /// expose bearer-authenticated routes over plaintext. Production ingress
    /// must terminate TLS in a reverse proxy and forward to this listener.
    pub async fn serve_with(&self, host: &str, port: u16) -> Result<(), DashboardError> {
        let listener = TcpListener::bind((host, port)).await?;
        let local_addr = listener.local_addr()?;
        if !local_addr.ip().is_loopback() {
            return Err(DashboardError::Other(format!(
                "refusing plaintext dashboard bind on non-loopback address {local_addr}; terminate TLS in a loopback reverse proxy"
            )));
        }
        if self.enrollment_only {
            // Nothing below this branch is started, because nothing below it
            // is reachable in this mode:
            //
            // * the seven Skarbiec boundary verifiers only gate object,
            //   release, machine, service, rate-limit and integration routes,
            //   all of which are refused by the allowlist. Skipping them also
            //   means this listener needs no vault at all, which is the point:
            //   it can run where the operator plane cannot.
            //
            // `boundaries` therefore stays all-false; no served route reads
            // it.
            //
            // This log is the operator's only confirmation of what they are
            // about to publish, so it names every served pair verbatim.
            eprintln!("[dashboard] enrollment-only listener on http://{local_addr}");
            eprintln!(
                "[dashboard] this listener serves ONLY the enrollment routes; every other path and method answers 404:"
            );
            for (method, path) in ENROLLMENT_ROUTES {
                eprintln!("[dashboard]   {method} {path}");
            }
            eprintln!(
                "[dashboard] no object, machine, service, host-health or integration route is served here"
            );
            return self.serve_on(listener).await;
        }
        // Every verifier reads shared Skarbiec vault/audit state. Starting all
        // boundaries together can overwhelm the listener and fail the whole
        // control plane on a transient connection reset, so validate them in
        // deterministic order with an independent timeout per boundary
        // ([`boundary_timeout`]).
        //
        // A verdict used to be recorded once and never revisited, so one slow
        // or reset read shut a boundary until somebody restarted the unit --
        // and `object` shutting means `503 object authorization unavailable`
        // for the whole fleet. That happened four times in one afternoon, each
        // time cured by an identical retry, so the retry belongs here instead
        // of in the operator's hands. The eager sweep below is that retry; the
        // inline recheck in [`Dashboard::recover_boundary`] is the other half,
        // because a boundary that resets an hour after startup never reaches
        // this code again.
        // Do not hold the listener behind this sweep. A slow upstream used to
        // leave the socket bound but unserved for minutes, so launchd and every
        // recovery client saw a timeout instead of the available `/healthz`
        // report. Routes remain closed until their own boundary is ready and
        // can revalidate it inline through `boundaries_available`.
        let validation = async {
            let attempts = std::env::var("WC_DASHBOARD_BOUNDARY_ATTEMPTS")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|count| *count > 0)
                .unwrap_or(3);
            let retry_pause = Duration::from_secs(2);
            for boundary in Boundary::ALL {
                let mut outcome = self.validate_boundary(boundary).await;
                let mut attempt = 1;
                while attempt < attempts && outcome.is_err() {
                    eprintln!(
                    "[dashboard] {} boundary attempt {attempt} of {attempts} did not settle; retrying",
                    boundary.label()
                );
                    tokio::time::sleep(retry_pause).await;
                    outcome = self.validate_boundary(boundary).await;
                    attempt += 1;
                }
                // Only `object` used to report why it failed, so every other
                // boundary said "unavailable" and left the operator guessing which
                // grant, item set or endpoint was at fault. The verdict is useless
                // without the reason, so the log carries the verifier's own words.
                if let Err(error) = &outcome {
                    eprintln!("[dashboard] {} boundary error: {error}", boundary.label());
                    eprintln!("[dashboard] {} boundary unavailable", boundary.label());
                }
                self.record_boundary(boundary, outcome);
            }
        };

        eprintln!("[dashboard] listening on http://{local_addr}");
        let serving = self.serve_on(listener);
        tokio::pin!(validation);
        tokio::pin!(serving);
        tokio::select! {
            result = &mut serving => result,
            _ = &mut validation => serving.await,
        }
    }

    /// Accept loop on an already-bound listener (tests bind 127.0.0.1:0).
    /// One task per connection — the ThreadingHTTPServer equivalent.
    pub async fn serve_on(&self, listener: TcpListener) -> Result<(), DashboardError> {
        let mut connections = FuturesUnordered::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let dashboard = self.clone();
                    connections.push(async move {
                        if let Err(exc) = dashboard.handle_connection(stream).await {
                            eprintln!("[dashboard] connection error: {exc}");
                        }
                    });
                }
                _ = connections.next(), if !connections.is_empty() => {}
            }
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        // The peer is the reverse proxy's loopback address behind an HTTPS
        // ingress; the invite routes only ever use it as a rate-limit bucket.
        let peer = stream.peer_addr().ok().map(|address| address.ip());
        let Some(mut request) = read_request(&mut stream).await? else {
            return Ok(());
        };
        request.peer = peer;
        // The mode gate is the FIRST thing that looks at the request, ahead of
        // the object PUT preflight, ahead of the remainder of the body, ahead
        // of every Host check and authorization, and ahead of any store or
        // vault access. A refused request costs one method/path comparison and
        // this listener never reads anything on its behalf; the rest of the
        // declared body is deliberately left unread, since the connection
        // closes anyway.
        if self.enrollment_only && !enrollment_route_allowed(&request.method, &request.path) {
            let response = Response::new(
                http_status("404"),
                "Not Found",
                "text/plain; charset=utf-8",
                ENROLLMENT_REFUSAL,
            );
            eprintln!(
                "[dashboard] \"{} {} HTTP/1.1\" {} enrollment-only",
                request.method, request.path, response.status
            );
            stream.write_all(&response.bytes).await?;
            return stream.shutdown().await;
        }
        let is_object_put = request.method == "PUT" && request.path.starts_with("/api/object?");
        if is_object_put {
            if let Some(response) = self.object_put_preflight(&request).await {
                stream.write_all(&response.bytes).await?;
                return stream.shutdown().await;
            }
        }
        if request.body.len() < request.content_length {
            let received = request.body.len();
            request.body.resize(request.content_length, u8::default());
            stream.read_exact(&mut request.body[received..]).await?;
        }
        let response = self.route(&request).await;
        eprintln!(
            "[dashboard] \"{} {} HTTP/1.1\" {} -",
            request.method, request.path, response.status
        );
        stream.write_all(&response.bytes).await?;
        stream.shutdown().await
    }

    async fn route(&self, request: &Request) -> Response {
        let path = request.path.split('?').next().unwrap_or("");
        if path.starts_with("/api/integration/") {
            if !self
                .trusted_request_host(request.header("host"), request.header("x-forwarded-proto"))
            {
                return send_json(http_status("403"), &json!({"error": "forbidden"}));
            }
            let available = self.boundaries_available(&[Boundary::Integration]).await;
            return integration::handle(request, available, &self.store).await;
        }
        match request.method.as_str() {
            "" => empty_response(400, "Bad Request"),
            "GET" => self.do_get(request).await,
            "POST" => self.do_post(request).await,
            "PUT" => self.do_put(request).await,
            "DELETE" => self.do_delete(request).await,
            // Python BaseHTTPRequestHandler: 501 Unsupported method.
            _ => empty_response(501, "Not Implemented"),
        }
    }
    async fn object_put_preflight(&self, request: &Request) -> Option<Response> {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return Some(send_json(
                http_status("403"),
                &json!({"error": "forbidden"}),
            ));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path != "/api/object" {
            return Some(empty_response(http_status("404"), "Not Found"));
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Some(response),
        };
        // A release coordinate authorizes against the RELEASE verifier's
        // material (`authorize_release` reads `release_token` for the mapped
        // item), so that boundary is this request's precondition just as much
        // as the object one. Requiring it here is what makes
        // `Boundary::Release` mean something: it was enumerated, labelled,
        // described as "release publication", validated once at startup and
        // required by NO route, so it read `false` until someone restarted the
        // unit and no request could ever reopen it — `boundaries_available`
        // revalidates only what a request requires. On 2026-08-31 an operator
        // read that field, believed its description, and held the quietest
        // publication window of the night waiting for a value with no
        // mechanism to change.
        //
        // Ordinary object traffic is deliberately unaffected: only a
        // release-policy coordinate adds the requirement, because only it
        // reads that material.
        // Ordinary object traffic is deliberately unaffected, and a release
        // coordinate now REVALIDATES the release boundary without being gated
        // by it -- the split `boundary_plan` exists for.
        let plan = boundary_plan(
            request.path.split('?').next().unwrap_or(""),
            Some((object.namespace(), object.key())),
        );
        if !self.satisfy_boundaries(&plan).await {
            return Some(send_json(
                http_status("503"),
                &json!({"error": "object authorization unavailable"}),
            ));
        }
        let authorized = if let Some(policy_key) =
            crate::object_store::release_policy_key(object.namespace(), object.key())
        {
            // A release object is only ever created, never replaced; the
            // client resolves its credential from the same routing function.
            let immutable = query_value(&parse_qs(query), "if_absent").as_deref() == Some("true");
            if object.namespace() == "releases" && !immutable {
                Ok(false)
            } else {
                authorize_release(self, request, &policy_key, false).await
            }
        } else {
            authorize_object(
                self,
                request,
                object.namespace(),
                object.key(),
                false,
                "put",
            )
            .await
        };
        match authorized {
            Ok(true) => {}
            Ok(false) => {
                return Some(send_json(
                    http_status("401"),
                    &json!({"error": "unauthorized or non-immutable release write"}),
                ))
            }
            Err(()) => {
                return Some(send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                ))
            }
        }
        None
    }

    async fn do_get(&self, request: &Request) -> Response {
        let path_no_query = request.path.split('?').next().unwrap_or("");
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            if path_no_query == "/api/machine/status" {
                return machine_result_response(Err(MachineError::new("FORBIDDEN", "forbidden")));
            }
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        if path_no_query == "/healthz" || path_no_query == "/livez" {
            // Liveness answers before authorization, so it publishes the flat
            // readiness booleans only: a boundary's reason names vault items,
            // grants and endpoints, and an unauthenticated probe has no
            // business reading those. The startup log carries the reason.
            let (degraded, boundaries) = {
                let boundaries = self
                    .boundaries
                    .read()
                    .expect("dashboard boundary state lock");
                (!boundaries.all_ready(), boundaries.ready_json())
            };
            return send_json(
                http_status("200"),
                &json!({
                    "ok": true,
                    "degraded": degraded,
                    "boundaries": boundaries,
                }),
            );
        }
        // The operator's read of the same state, with each boundary's reason.
        //
        // `/healthz` cannot carry it and should not, and until this route
        // existed the sentence was reachable nowhere on a live process: the
        // verdict is held in memory, the holder logs no boundary line unless
        // it revalidates, and the standing remedy points at a unit log that on
        // one host does not exist. So a closed boundary was one bit, and one
        // bit cannot distinguish `validation did not settle within N seconds`
        // — arithmetic, answered by the item budget — from `item set mismatch`
        // or `missing or empty`, which is a credential answer and is not fixed
        // by restarting anything.
        //
        // Loopback-only, like every route on this listener, and it publishes
        // the verifier's own sentence rather than any material: what refused
        // and about which subject.
        if path_no_query == "/api/state.json" {
            let (degraded, boundaries) = {
                let boundaries = self
                    .boundaries
                    .read()
                    .expect("dashboard boundary state lock");
                (!boundaries.all_ready(), boundaries.state_json())
            };
            return send_json(
                http_status("200"),
                &json!({
                    "degraded": degraded,
                    "boundaries": boundaries,
                }),
            );
        }
        // Enrollment by invite. Both answer before any operator
        // authorization is consulted, and neither ever consults it: the
        // machine holds an invite code and nothing else, and a loopback
        // caller's implicit operator trust must not become a way in.
        if path_no_query == "/join.sh" {
            return fleet_join::join_script();
        }
        if path_no_query == "/api/fleet/invite/key" {
            return fleet_join::invite_key(&self.store, request).await;
        }
        let release_object_route = path_no_query == "/api/release/object";
        let object_route = path_no_query == "/api/object"
            || path_no_query == "/api/object/list"
            || path_no_query == "/api/object/stat";
        if object_route {
            let query = request
                .path
                .split_once('?')
                .map(|(_, query)| query)
                .unwrap_or("");
            let scope = if path_no_query == "/api/object/list" {
                object_list_from_query(query).map(|(namespace, prefix)| (namespace, prefix, true))
            } else {
                object_from_query(query).map(|object| {
                    (
                        object.namespace().to_string(),
                        object.key().to_string(),
                        false,
                    )
                })
            };
            let (namespace, key_or_prefix, list) = match scope {
                Ok(scope) => scope,
                Err(response) => return response,
            };
            // Same rule as the writer above, and the same split: a release
            // coordinate revalidates the release boundary, which is what
            // gives it a way to reopen, without being gated by it.
            let plan = boundary_plan(path_no_query, Some((&namespace, &key_or_prefix)));
            if !self.satisfy_boundaries(&plan).await {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                );
            }
            let action = if list {
                "list"
            } else if path_no_query == "/api/object/stat" {
                "stat"
            } else {
                "get"
            };
            let authorized = if let Some(policy_key) =
                crate::object_store::release_policy_key(&namespace, &key_or_prefix)
            {
                // A catalog object is addressed exactly, never listed as a prefix.
                let listing = list && namespace != "system";
                authorize_release(self, request, &policy_key, listing).await
            } else {
                authorize_object(self, request, &namespace, &key_or_prefix, list, action).await
            };
            match authorized {
                Ok(true) => {}
                Ok(false) => {
                    return send_json(http_status("401"), &json!({"error": "unauthorized"}))
                }
                Err(()) => {
                    return send_json(
                        http_status("503"),
                        &json!({"error": "object authorization unavailable"}),
                    )
                }
            }
        } else if !release_object_route {
            // At most one of these two paths matches, so at most one boundary
            // is ever revalidated here.
            if path_no_query == "/api/service/status"
                && !self.boundaries_available(&[Boundary::Service]).await
            {
                return send_json(
                    http_status("503"),
                    &json!({"error": "service authorization unavailable"}),
                );
            }
            if path_no_query == "/api/machine/status"
                && !self.boundaries_available(&[Boundary::Machine]).await
            {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )));
            }
            if path_no_query == "/api/service/status" {
                let query = request
                    .path
                    .split_once('?')
                    .map(|(_, query)| query)
                    .unwrap_or("");
                let service = match service_name(query) {
                    Ok(service) => service,
                    Err(response) => return response,
                };
                match authorize_service(request, service, "status").await {
                    Ok(true) => {}
                    Ok(false) => {
                        return send_json(http_status("401"), &json!({"error": "unauthorized"}))
                    }
                    Err(()) => {
                        return send_json(
                            http_status("503"),
                            &json!({"error": "service authorization unavailable"}),
                        )
                    }
                }
            }
        }
        match self.get_routes(request).await {
            Ok(response) => response,
            Err(_) => Response::text(
                http_status("500"),
                "Internal Server Error",
                "dashboard error",
            ),
        }
    }

    async fn get_routes(&self, request: &Request) -> Result<Response, DashboardError> {
        let (path, query) = match request.path.split_once('?') {
            Some((path, query)) => (path, query),
            None => (request.path.as_str(), ""),
        };
        if path == "/api/release/object" {
            // Public read-only release channel. This dashboard route is the
            // store's delivery endpoint; an operator-owned TLS reverse proxy
            // may expose it off-host without changing its response contract:
            //   200: the object bytes, Content-Type from object metadata
            //        (default application/octet-stream), Accept-Ranges: bytes
            //   206/416: byte-range answers from get_object
            //   400: {"error": ...} — missing or unparseable uri
            //   403: {"error": ...} — a namespace that is not `releases`
            //   404: {"state": "absent", "uri": ...} — the object is missing
            let object = match object_from_query(query) {
                Ok(object) => object,
                Err(response) => return Ok(response),
            };
            if object.namespace() != "releases" {
                return Ok(send_json(
                    http_status("403"),
                    &json!({"error": "only stado://releases software artifacts are publicly readable"}),
                ));
            }
            return self.get_object(request, query).await;
        }
        if path == "/api/object" {
            return self.get_object(request, query).await;
        }
        if path == "/api/object/list" {
            return self.list_objects(query).await;
        }
        if path == "/api/object/stat" {
            return self.stat_object(query).await;
        }
        if path == "/api/machine/status" {
            return Ok(self.get_machine_status(request, query).await);
        }
        if path == "/api/service/status" {
            return Ok(self.get_service_status(request, query).await);
        }
        // The registry-policy boundary gates both of these. Its verifier is
        // ready even when nothing is declared, so an undeclared deployment
        // refuses here with 401 rather than reporting an outage.
        if path == "/api/registry.json" {
            if !self.boundaries_available(&[Boundary::Registry]).await {
                return Ok(send_json(
                    http_status("503"),
                    &json!({"error": "registry authorization unavailable"}),
                ));
            }
            if let Err(response) = registry_policy::authorized(request, "policy-read").await {
                return Ok(response);
            }
            return Ok(registry_policy::get_policy().await);
        }
        if path == "/api/cleanup.json" {
            if !self.boundaries_available(&[Boundary::Registry]).await {
                return Ok(send_json(
                    http_status("503"),
                    &json!({"error": "registry authorization unavailable"}),
                ));
            }
            if let Err(response) = registry_policy::authorized(request, "cleanup-read").await {
                return Ok(response);
            }
            return Ok(registry_policy::get_cleanup());
        }

        Ok(empty_response(404, "Not Found"))
    }

    async fn get_object(&self, request: &Request, query: &str) -> Result<Response, DashboardError> {
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Ok(response),
        };
        let path = object.storage_path();
        let versioned = query_value(&parse_qs(query), "versioned").as_deref() == Some("true");
        let (bytes, version) = if versioned {
            let Some(value) = self.store.read_text_versioned(&path).await? else {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            };
            (value.content.into_bytes(), Some(value.version))
        } else {
            let public_release_route = request
                .path
                .split_once('?')
                .map_or(request.path.as_str(), |(path, _)| path)
                == "/api/release/object";
            let bytes = if object.namespace() == "releases" && public_release_route {
                // Public delivery may traverse a namespaced Stado-object backend,
                // so it uses that backend's cross-namespace release route.
                self.store
                    .backend()
                    .download_release(&object.to_string())
                    .await?
            } else {
                // Authenticated /api/object reads the same local storage path as
                // PUT. Sending this path through the public route made a successful
                // write immediately unreadable and broke publisher preflight.
                self.store.read_bytes(&path).await?
            };
            let Some(bytes) = bytes else {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            };
            (bytes, None)
        };
        let metadata = self
            .store
            .backend()
            .list_blobs_with_meta(&path)
            .await?
            .into_iter()
            .find(|blob| blob.name == path)
            .map(|blob| blob.metadata)
            .unwrap_or_default();
        let content_type = metadata
            .get("content-type")
            .map(String::as_str)
            .unwrap_or("application/octet-stream");
        if let Some(value) = request.header("range") {
            let Some((start, end)) = parse_byte_range(value, bytes.len()) else {
                return Ok(Response::new_with_headers(
                    http_status("416"),
                    "Range Not Satisfiable",
                    content_type,
                    b"",
                    &[("Content-Range", format!("bytes */{}", bytes.len()))],
                ));
            };
            return Ok(Response::new_with_headers(
                http_status("206"),
                "Partial Content",
                content_type,
                &bytes[start..=end],
                &[
                    ("Accept-Ranges", "bytes".to_string()),
                    (
                        "Content-Range",
                        format!("bytes {start}-{end}/{}", bytes.len()),
                    ),
                ],
            ));
        }
        let mut headers = vec![("Accept-Ranges", "bytes".to_string())];
        if let Some(version) = version {
            headers.push(("X-Stado-Version", version));
        }
        Ok(Response::new_with_headers(
            http_status("200"),
            "OK",
            content_type,
            &bytes,
            &headers,
        ))
    }

    async fn list_objects(&self, query: &str) -> Result<Response, DashboardError> {
        let (namespace, requested_prefix) = match object_list_from_query(query) {
            Ok(scope) => scope,
            Err(response) => return Ok(response),
        };
        let prefix = if release_object_namespace(&namespace) {
            config::release_publisher_for_list(&requested_prefix).map(|(_, authorized)| authorized)
        } else {
            config::object_api_namespace(&namespace)
                .and_then(|policy| policy.authorized_list_prefix(&requested_prefix, "list"))
        };
        let Some(prefix) = prefix else {
            return Ok(send_json(
                http_status("401"),
                &json!({"error": "unauthorized"}),
            ));
        };
        let storage_prefix = crate::object_store::ObjectRef::namespace_prefix(&namespace, &prefix)?;
        let objects = self
            .store
            .backend()
            .list_blobs_with_meta(&storage_prefix)
            .await?;
        let mut response = Vec::with_capacity(objects.len());
        for blob in objects {
            let object = crate::object_store::ObjectRef::from_storage_path(&blob.name)?;
            response.push(json!({
                "uri": object.to_string(),
                "namespace": object.namespace(),
                "key": object.key(),
                "size": blob.size,
                "updated_at": blob.updated.map(isoformat_utc),
                "metadata": blob.metadata,
            }));
        }
        Ok(send_json(http_status("200"), &json!({"objects": response})))
    }

    async fn stat_object(&self, query: &str) -> Result<Response, DashboardError> {
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Ok(response),
        };
        let path = object.storage_path();
        let blob = self
            .store
            .backend()
            .list_blobs_with_meta(&path)
            .await?
            .into_iter()
            .find(|blob| blob.name == path);
        Ok(match blob {
            Some(blob) => send_json(
                http_status("200"),
                &json!({
                    "state": "present",
                    "uri": object.to_string(),
                    "size": blob.size,
                    "updated_at": blob.updated.map(isoformat_utc),
                    "metadata": blob.metadata,
                }),
            ),
            None => send_json(
                http_status("404"),
                &json!({"state": "absent", "uri": object.to_string()}),
            ),
        })
    }

    fn machine_facade(&self) -> MachineFacade {
        MachineFacade::with_store(self.store.clone(), self.store.bucket_name().to_string())
    }

    async fn get_machine_status(&self, request: &Request, query: &str) -> Response {
        if request.content_length != usize::default() || !request.body.is_empty() {
            return invalid_machine_request("machine status does not accept a request body");
        }
        let client = match authenticate_machine_client(request, "status").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let job_id = match machine_job_id(query) {
            Ok(job_id) => job_id,
            Err(response) => return response,
        };
        let result = self.machine_facade().status(job_id).await;
        let target_allowed = result
            .as_ref()
            .ok()
            .and_then(machine_result_target)
            .is_some_and(|target| client.allows_target(target));
        if !target_allowed {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        }
        machine_result_response(result)
    }

    async fn post_machine_submit(&self, request: &Request) -> Response {
        let content_type = request
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if request.path != "/api/machine/submit"
            || content_type != "application/json"
            || request.header("transfer-encoding").is_some()
            || request.header("content-length").is_none()
            || request.content_length != request.body.len()
            || request.body.len() > MAX_HEAD_BYTES
        {
            return invalid_machine_request("invalid JSON request framing");
        }
        let mut payload: Value = match serde_json::from_slice(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return invalid_machine_request(format!("cannot read request JSON: {error}"))
            }
        };
        if let Err(error) = validate_remote_machine_request(&payload) {
            return machine_result_response(Err(error));
        }
        let client = match authenticate_machine_client(request, "submit").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let requested = payload
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = if requested.is_empty() {
            let [target] = client.targets() else {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )));
            };
            target.clone()
        } else if client.allows_target(requested) {
            requested.to_string()
        } else {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        };
        let Some(object) = payload.as_object_mut() else {
            return invalid_machine_request("machine request must be an object");
        };
        object.insert("provider".to_string(), Value::String(target));
        object.insert("pin_to_provider".to_string(), Value::Bool(true));
        machine_result_response(self.machine_facade().submit_request(&payload).await)
    }

    async fn post_machine_cancel(&self, request: &Request, query: &str) -> Response {
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return invalid_machine_request("machine cancel does not accept a request body");
        }
        let client = match authenticate_machine_client(request, "cancel").await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return machine_result_response(Err(MachineError::new(
                    "UNAUTHORIZED",
                    "unauthorized",
                )))
            }
            Err(()) => {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )))
            }
        };
        let job_id = match machine_job_id(query) {
            Ok(job_id) => job_id,
            Err(response) => return response,
        };
        let status = self.machine_facade().status(job_id).await;
        let target_allowed = status
            .as_ref()
            .ok()
            .and_then(machine_result_target)
            .is_some_and(|target| client.allows_target(target));
        if !target_allowed {
            return machine_result_response(Err(MachineError::new("UNAUTHORIZED", "unauthorized")));
        }
        machine_result_response(self.machine_facade().cancel_job(job_id).await)
    }

    async fn get_service_status(&self, request: &Request, query: &str) -> Response {
        if request.content_length != usize::default() || !request.body.is_empty() {
            return invalid_service_request("service status does not accept a request body");
        }
        let name = match service_name(query) {
            Ok(name) => name,
            Err(response) => return response,
        };
        let store = match service_beacon_store().await {
            Ok(store) => store,
            Err(message) => {
                return service_failure(http_status("503"), "SERVICE_STATUS_FAILED", message, true)
            }
        };
        let rows = match service::find_services(&store, name).await {
            Ok(rows) => rows,
            Err(error) => {
                return service_failure(
                    http_status("503"),
                    "SERVICE_STATUS_FAILED",
                    error.to_string(),
                    true,
                )
            }
        };
        if rows.is_empty() {
            return service_failure(
                http_status("404"),
                "NOT_FOUND",
                format!("no registry-managed service named {name}"),
                false,
            );
        }
        service_success(Value::Array(
            rows.iter().map(service::ServiceStatus::to_json).collect(),
        ))
    }

    async fn post_service_restart(&self, request: &Request, query: &str) -> Response {
        if request.header("transfer-encoding").is_some()
            || request.content_length != usize::default()
            || !request.body.is_empty()
        {
            return invalid_service_request("service restart does not accept a request body");
        }
        let name = match service_name(query) {
            Ok(name) => name,
            Err(response) => return response,
        };
        let services = match declared_services_matching(name).await {
            Ok(services) => services,
            Err(message) => {
                return service_failure(http_status("503"), "SERVICE_RESTART_FAILED", message, true)
            }
        };
        if services.is_empty() {
            return service_failure(
                http_status("404"),
                "NOT_FOUND",
                format!("no registry-managed service named {name}"),
                false,
            );
        }
        let runner = production_runner();
        let mut result = Vec::with_capacity(services.len());
        let mut failures = Vec::new();
        for declared in &services {
            let target = match host_channel::canonical_target(&declared.host).await {
                Ok(target) => target,
                Err(error) => {
                    return service_failure(
                        http_status("503"),
                        "SERVICE_RESTART_FAILED",
                        error.to_string(),
                        true,
                    )
                }
            };
            let report = match service::restart_service(&target, declared, &runner).await {
                Ok(report) => report,
                Err(error) => {
                    return service_failure(
                        http_status("503"),
                        "SERVICE_RESTART_FAILED",
                        error.to_string(),
                        true,
                    )
                }
            };
            if !report.succeeded("restarted") {
                failures.push(format!("{}: {}", declared.host, report.failure()));
            }
            let mut entry = report.to_json();
            entry["host"] = Value::from(declared.host.clone());
            result.push(entry);
        }
        if !failures.is_empty() {
            return service_failure(
                http_status("503"),
                "SERVICE_RESTART_FAILED",
                format!("restart failed on {}", failures.join("; ")),
                true,
            );
        }
        service_success(Value::Array(result))
    }

    async fn put_object(
        &self,
        request: &Request,
        object: &crate::object_store::ObjectRef,
        query: &str,
    ) -> Result<Response, DashboardError> {
        let values = parse_qs(query);
        let if_absent = query_value(&values, "if_absent").as_deref() == Some("true");
        let if_version = query_value(&values, "if_version").filter(|value| !value.is_empty());
        let metadata_only = query_value(&values, "metadata_only").as_deref() == Some("true");
        let selected =
            usize::from(if_absent) + usize::from(if_version.is_some()) + usize::from(metadata_only);
        if selected > 1 {
            return Ok(send_json(
                http_status("400"),
                &json!({"error": "if_absent, if_version, and metadata_only are mutually exclusive"}),
            ));
        }
        let path = object.storage_path();
        if metadata_only {
            if !self.store.backend().exists(&path).await? {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            }
            let metadata: std::collections::BTreeMap<String, String> =
                match serde_json::from_slice(&request.body) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        return Ok(send_json(
                            http_status("400"),
                            &json!({"error": format!("invalid metadata: {error}")}),
                        ))
                    }
                };
            self.store.backend().set_metadata(&path, &metadata).await?;
            return Ok(send_json(
                http_status("200"),
                &json!({"state": "metadata-updated", "uri": object.to_string()}),
            ));
        }
        if let Some(expected_version) = if_version {
            let content = match std::str::from_utf8(&request.body) {
                Ok(content) => content,
                Err(error) => {
                    return Ok(send_json(
                        http_status("400"),
                        &json!({"error": format!("conditional object writes require UTF-8: {error}")}),
                    ))
                }
            };
            let version = match self
                .store
                .compare_and_swap_text(&path, &expected_version, content)
                .await
            {
                Ok(version) => version,
                Err(StorageError::StorageConflict(_)) => {
                    return Ok(send_json(
                        http_status("409"),
                        &json!({"error": "object version changed", "uri": object.to_string()}),
                    ))
                }
                Err(StorageError::NotFound(_)) => {
                    return Ok(send_json(
                        http_status("404"),
                        &json!({"state": "absent", "uri": object.to_string()}),
                    ))
                }
                Err(error) => return Err(error.into()),
            };
            return Ok(send_json(
                http_status("200"),
                &json!({
                    "state": "stored",
                    "uri": object.to_string(),
                    "version": version,
                }),
            ));
        }
        let content_type = request
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .to_string();
        let extra = match request.header("x-stado-object-metadata") {
            Some(raw) => match serde_json::from_str(raw) {
                Ok(value) => value,
                Err(error) => {
                    return Ok(send_json(
                        http_status("400"),
                        &json!({"error": format!("invalid object metadata: {error}")}),
                    ))
                }
            },
            None => BTreeMap::new(),
        };
        let metadata = match merged_object_metadata(object, &content_type, &extra) {
            Ok(metadata) => metadata,
            Err(error) => return Ok(send_json(http_status("400"), &json!({"error": error}))),
        };
        if if_absent {
            let mut source = tempfile::NamedTempFile::new()?;
            source.write_all(&request.body)?;
            if !self
                .store
                .upload_file_if_absent(&path, source.path())
                .await?
            {
                return Ok(send_json(
                    http_status("409"),
                    &json!({"error": "object exists", "uri": object.to_string()}),
                ));
            }
        } else {
            self.store.upload_bytes(&path, &request.body).await?;
        }
        self.store.backend().set_metadata(&path, &metadata).await?;
        let landed = self.store.backend().list_blobs_with_meta(&path).await?;
        let Some(blob) = landed.into_iter().find(|blob| blob.name == path) else {
            return Err(DashboardError::Other(format!(
                "object metadata verification could not find {object}"
            )));
        };
        if metadata
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .any(|(key, value)| blob.metadata.get(key) != Some(value))
        {
            return Err(DashboardError::Other(format!(
                "object metadata verification failed for {object}"
            )));
        }
        Ok(send_json(
            http_status("200"),
            &json!({
                "state": "stored",
                "uri": object.to_string(),
                "content_type": content_type,
            }),
        ))
    }

    async fn post_object_compose(&self, request: &Request) -> Response {
        let request_content_type = request
            .header("content-type")
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if request_content_type != Some("application/json") {
            return object_compose_error(
                http_status("415"),
                "content-type must be application/json",
            );
        }
        let payload = match serde_json::from_slice::<ObjectComposeRequest>(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return object_compose_error(
                    http_status("400"),
                    format!("invalid object composition request: {error}"),
                )
            }
        };
        let object = match ObjectRef::parse(&payload.uri) {
            Ok(object) => object,
            Err(error) => return object_compose_error(http_status("400"), error.to_string()),
        };
        if object.to_string() != payload.uri {
            return object_compose_error(
                http_status("400"),
                "composition uri must use the canonical stado:// form",
            );
        }
        if object.key().contains(".__stado_upload/") {
            return object_compose_error(
                http_status("400"),
                "a staged upload cannot be a composition target",
            );
        }
        if payload.content_type.is_empty()
            || payload.content_type.len() > MAX_HEAD_BYTES
            || payload.content_type.chars().any(char::is_control)
        {
            return object_compose_error(http_status("400"), "invalid object content type");
        }
        let expected_upload_digest = match parse_sha256(&payload.upload_id) {
            Some(digest) => digest,
            None => {
                return object_compose_error(
                    http_status("400"),
                    "upload_id must be a lowercase SHA-256 digest",
                )
            }
        };
        if payload.size == 0 || payload.size > crate::object_store::max_object_bytes() {
            return object_compose_error(
                http_status("400"),
                "composition size is outside the object API limit",
            );
        }
        let expected_chunk_count = payload.size.div_ceil(OBJECT_API_CHUNK_BYTES);
        if payload.chunks.len() != expected_chunk_count {
            return object_compose_error(
                http_status("400"),
                format!(
                    "composition requires {expected_chunk_count} contiguous chunks for {} bytes",
                    payload.size
                ),
            );
        }

        let mut declared_total = 0usize;
        let mut chunks = Vec::with_capacity(payload.chunks.len());
        for (index, chunk) in payload.chunks.iter().enumerate() {
            let expected_size = payload
                .size
                .saturating_sub(declared_total)
                .min(OBJECT_API_CHUNK_BYTES);
            if chunk.size != expected_size {
                return object_compose_error(
                    http_status("400"),
                    format!("chunk {index} must declare exactly {expected_size} bytes"),
                );
            }
            declared_total = match declared_total.checked_add(chunk.size) {
                Some(total) => total,
                None => {
                    return object_compose_error(
                        http_status("400"),
                        "composition chunk sizes overflow",
                    )
                }
            };
            let expected_digest = match parse_sha256(&chunk.sha256) {
                Some(digest) => digest,
                None => {
                    return object_compose_error(
                        http_status("400"),
                        format!("chunk {index} sha256 must be a lowercase SHA-256 digest"),
                    )
                }
            };
            let chunk_object = match ObjectRef::parse(&chunk.uri) {
                Ok(object) => object,
                Err(error) => {
                    return object_compose_error(
                        http_status("400"),
                        format!("invalid chunk {index} uri: {error}"),
                    )
                }
            };
            let expected_key = format!(
                "{}.__stado_upload/{}/{index:08}",
                object.key(),
                payload.upload_id
            );
            if chunk_object.to_string() != chunk.uri
                || chunk_object.namespace() != object.namespace()
                || chunk_object.key() != expected_key
            {
                return object_compose_error(
                    http_status("400"),
                    format!("chunk {index} is outside the target upload"),
                );
            }
            chunks.push((chunk_object, expected_digest));
        }
        if declared_total != payload.size {
            return object_compose_error(
                http_status("400"),
                "composition chunk sizes do not equal the declared object size",
            );
        }
        let metadata =
            match merged_object_metadata(&object, &payload.content_type, &payload.metadata) {
                Ok(metadata) => metadata,
                Err(error) => return object_compose_error(http_status("400"), error),
            };

        if requires_object_boundary(object.namespace(), object.key())
            && !self.boundaries_available(&[Boundary::Object]).await
        {
            return object_compose_error(http_status("503"), "object authorization unavailable");
        }
        let authorized = if let Some(policy_key) =
            crate::object_store::release_policy_key(object.namespace(), object.key())
        {
            if object.namespace() == "releases" && !payload.if_absent {
                Ok(false)
            } else {
                authorize_release(self, request, &policy_key, false).await
            }
        } else {
            authorize_object(
                self,
                request,
                object.namespace(),
                object.key(),
                false,
                "put",
            )
            .await
        };
        match authorized {
            Ok(true) => {}
            Ok(false) => {
                return object_compose_error(
                    http_status("401"),
                    "unauthorized or non-immutable release write",
                )
            }
            Err(()) => {
                return object_compose_error(http_status("503"), "object authorization unavailable")
            }
        }

        let mut staged = match tempfile::NamedTempFile::new() {
            Ok(staged) => staged,
            Err(error) => return object_compose_error(http_status("500"), error.to_string()),
        };
        let mut object_digest = Sha256::new();
        let mut assembled_size = 0usize;
        for ((chunk_object, expected_digest), declared) in chunks.iter().zip(&payload.chunks) {
            let bytes = match self.store.read_bytes(&chunk_object.storage_path()).await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => {
                    return object_compose_response(
                        http_status("404"),
                        json!({"state": "absent", "uri": chunk_object.to_string()}),
                    )
                }
                Err(error) => return object_compose_error(http_status("500"), error.to_string()),
            };
            let actual_digest: [u8; 32] = Sha256::digest(&bytes).into();
            if bytes.len() != declared.size || actual_digest != *expected_digest {
                return object_compose_error(
                    http_status("422"),
                    format!("stored chunk does not match {}", chunk_object),
                );
            }
            if let Err(error) = staged.write_all(&bytes) {
                return object_compose_error(http_status("500"), error.to_string());
            }
            object_digest.update(&bytes);
            assembled_size += bytes.len();
        }
        if assembled_size != payload.size {
            return object_compose_error(
                http_status("422"),
                "assembled object size differs from the composition request",
            );
        }
        let assembled_digest: [u8; 32] = object_digest.finalize().into();
        if assembled_digest != expected_upload_digest {
            return object_compose_error(
                http_status("422"),
                "assembled object SHA-256 differs from upload_id",
            );
        }
        if let Err(error) = staged.flush() {
            return object_compose_error(http_status("500"), error.to_string());
        }

        let target_path = object.storage_path();
        if payload.if_absent {
            let created = match self
                .store
                .upload_file_if_absent(&target_path, staged.path())
                .await
            {
                Ok(created) => created,
                Err(error) => return object_compose_error(http_status("500"), error.to_string()),
            };
            if !created {
                let existing = match self.store.read_bytes(&target_path).await {
                    Ok(Some(existing)) => existing,
                    Ok(None) => {
                        return object_compose_error(
                            http_status("500"),
                            "object disappeared after create-only conflict",
                        )
                    }
                    Err(error) => {
                        return object_compose_error(http_status("500"), error.to_string())
                    }
                };
                let existing_digest: [u8; 32] = Sha256::digest(&existing).into();
                if existing.len() != payload.size || existing_digest != expected_upload_digest {
                    return object_compose_response(
                        http_status("409"),
                        json!({
                            "error": "object exists with different content",
                            "uri": object.to_string(),
                        }),
                    );
                }
            }
        } else {
            let bytes = match std::fs::read(staged.path()) {
                Ok(bytes) => bytes,
                Err(error) => return object_compose_error(http_status("500"), error.to_string()),
            };
            if let Err(error) = self.store.upload_bytes(&target_path, &bytes).await {
                return object_compose_error(http_status("500"), error.to_string());
            }
        }

        if let Err(error) = self
            .store
            .backend()
            .set_metadata(&target_path, &metadata)
            .await
        {
            return object_compose_error(http_status("500"), error.to_string());
        }
        let landed = match self
            .store
            .backend()
            .list_blobs_with_meta(&target_path)
            .await
        {
            Ok(landed) => landed,
            Err(error) => return object_compose_error(http_status("500"), error.to_string()),
        };
        let Some(blob) = landed.into_iter().find(|blob| blob.name == target_path) else {
            return object_compose_error(
                http_status("500"),
                format!("object metadata verification could not find {object}"),
            );
        };
        if metadata
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .any(|(key, value)| blob.metadata.get(key) != Some(value))
        {
            return object_compose_error(
                http_status("500"),
                format!("object metadata verification failed for {object}"),
            );
        }

        let cleanup_paths = if payload.if_absent {
            let prefix = format!("{target_path}.__stado_upload/");
            match self.store.list_paths(&prefix, usize::default()).await {
                Ok(paths) => paths
                    .into_iter()
                    .filter(|path| {
                        ObjectRef::from_storage_path(path).is_ok_and(|candidate| {
                            candidate.namespace() == object.namespace()
                                && release_upload_target_key(candidate.key()) == Some(object.key())
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(error) => return object_compose_error(http_status("500"), error.to_string()),
            }
        } else {
            chunks
                .iter()
                .map(|(chunk, _)| chunk.storage_path())
                .collect::<Vec<_>>()
        };
        for chunk_path in cleanup_paths {
            if let Err(error) = self.store.delete_blob(&chunk_path).await {
                return object_compose_error(http_status("500"), error.to_string());
            }
        }

        object_compose_response(
            http_status("200"),
            json!({
                "state": "stored",
                "uri": object.to_string(),
                "content_type": payload.content_type,
            }),
        )
    }

    async fn post_rate_limit_consume(&self, request: &Request) -> Response {
        let content_type = request
            .header("content-type")
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if content_type != Some("application/json") {
            return send_json(
                http_status("415"),
                &json!({"error": "content-type must be application/json"}),
            );
        }
        let supplied = request
            .header("authorization")
            .and_then(|value| value.trim().strip_prefix("Bearer "))
            .unwrap_or_default();
        let client = match rate_limit::authenticate(supplied).await {
            Ok(Some(client)) => client,
            Ok(None) => return send_json(http_status("401"), &json!({"error": "unauthorized"})),
            Err(_) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "rate limiting unavailable"}),
                )
            }
        };
        let payload = match serde_json::from_slice::<ConsumeRequest>(&request.body) {
            Ok(payload) => payload,
            Err(_) => {
                return send_json(
                    http_status("400"),
                    &json!({"error": "invalid rate-limit request"}),
                )
            }
        };
        match self.rate_limiter.consume(client, &payload).await {
            Ok(response) => send_json(http_status("200"), &json!(response)),
            Err(RateLimitError::InvalidRequest(message)) => {
                send_json(http_status("400"), &json!({"error": message}))
            }
            Err(_) => send_json(
                http_status("503"),
                &json!({"error": "rate limiting unavailable"}),
            ),
        }
    }

    async fn do_post(&self, request: &Request) -> Response {
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        let control_route = path == "/api/machine/submit"
            || path == "/api/machine/cancel"
            || path == "/api/service/restart"
            || path == "/api/rate-limit/consume";
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            if matches!(path, "/api/machine/submit" | "/api/machine/cancel") {
                return machine_result_response(Err(MachineError::new("FORBIDDEN", "forbidden")));
            }
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        // Enrollment by invite: authorized by the invite token alone, before
        // any operator authorization is reached, and never by it.
        if path == "/api/fleet/join" {
            return fleet_join::join(&self.store, request).await;
        }
        if path == "/api/object/compose" {
            return self.post_object_compose(request).await;
        }
        let required: &[Boundary] = match path {
            "/api/rate-limit/consume" => &[Boundary::RateLimitVerifier, Boundary::RateLimitState],
            "/api/machine/submit" | "/api/machine/cancel" => &[Boundary::Machine],
            "/api/service/restart" => &[Boundary::Service],
            _ => &[],
        };
        let unavailable = !self.boundaries_available(required).await;
        if unavailable {
            if matches!(path, "/api/machine/submit" | "/api/machine/cancel") {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )));
            }
            return send_json(
                http_status("503"),
                &json!({"error": "authorization boundary unavailable"}),
            );
        }
        if path == "/api/rate-limit/consume" {
            return self.post_rate_limit_consume(request).await;
        }
        // Registry-policy writes and janitor runs. Gated on their own
        // boundary, so a deployment that has declared no client refuses them
        // with 401 while every other route on this listener is unaffected.
        if matches!(path, "/api/registry/policy" | "/api/cleanup/run") {
            if !self.boundaries_available(&[Boundary::Registry]).await {
                return send_json(
                    http_status("503"),
                    &json!({"error": "registry authorization unavailable"}),
                );
            }
            let action = if path == "/api/registry/policy" {
                "policy-write"
            } else {
                "cleanup-run"
            };
            if let Err(response) = registry_policy::authorized(request, action).await {
                return response;
            }
            return if path == "/api/registry/policy" {
                registry_policy::set_policy(request).await
            } else {
                registry_policy::run_cleanup().await
            };
        }
        if control_route {
            if path == "/api/service/restart" {
                let service = match service_name(query) {
                    Ok(service) => service,
                    Err(response) => return response,
                };
                match authorize_service(request, service, "restart").await {
                    Ok(true) => {}
                    Ok(false) => {
                        return send_json(http_status("401"), &json!({"error": "unauthorized"}))
                    }
                    Err(()) => {
                        return send_json(
                            http_status("503"),
                            &json!({"error": "service authorization unavailable"}),
                        )
                    }
                }
                return self.post_service_restart(request, query).await;
            }
            return if path == "/api/machine/submit" {
                self.post_machine_submit(request).await
            } else {
                self.post_machine_cancel(request, query).await
            };
        }
        empty_response(http_status("404"), "Not Found")
    }

    async fn do_put(&self, request: &Request) -> Response {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path == "/api/host-health" {
            return self.put_host_health(request, query).await;
        }
        if path != "/api/object" {
            return empty_response(http_status("404"), "Not Found");
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return response,
        };
        if requires_object_boundary(object.namespace(), object.key())
            && !self.boundaries_available(&[Boundary::Object]).await
        {
            return send_json(
                http_status("503"),
                &json!({"error": "object authorization unavailable"}),
            );
        }
        let authorized = if let Some(policy_key) =
            crate::object_store::release_policy_key(object.namespace(), object.key())
        {
            // A release object is only ever created, never replaced; the
            // client resolves its credential from the same routing function.
            let immutable = query_value(&parse_qs(query), "if_absent").as_deref() == Some("true");
            if object.namespace() == "releases" && !immutable {
                Ok(false)
            } else {
                authorize_release(self, request, &policy_key, false).await
            }
        } else {
            authorize_object(
                self,
                request,
                object.namespace(),
                object.key(),
                false,
                "put",
            )
            .await
        };
        match authorized {
            Ok(true) => {}
            Ok(false) => {
                return send_json(
                    http_status("401"),
                    &json!({"error": "unauthorized or non-immutable release write"}),
                )
            }
            Err(()) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                )
            }
        }
        match self.put_object(request, &object, query).await {
            Ok(response) => response,
            Err(error) => send_json(http_status("500"), &json!({"error": error.to_string()})),
        }
    }

    async fn do_delete(&self, request: &Request) -> Response {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path != "/api/object" {
            return empty_response(http_status("404"), "Not Found");
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return response,
        };
        let authorized = if release_object_namespace(object.namespace()) {
            let Some(target_key) = release_upload_target_key(object.key()) else {
                return send_json(
                    http_status("403"),
                    &json!({"error": "release objects are immutable and cannot be deleted"}),
                );
            };
            authorize_release(self, request, target_key, false).await
        } else {
            if !self.boundaries_available(&[Boundary::Object]).await {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                );
            }
            authorize_object(
                self,
                request,
                object.namespace(),
                object.key(),
                false,
                "delete",
            )
            .await
        };
        match authorized {
            Ok(true) => {}
            Ok(false) => return send_json(http_status("401"), &json!({"error": "unauthorized"})),
            Err(()) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                )
            }
        }
        let result = self.store.delete_blob(&object.storage_path()).await;
        match result {
            Ok(()) => send_json(
                http_status("200"),
                &json!({"state": "absent", "uri": object.to_string()}),
            ),
            Err(error) => send_json(http_status("500"), &json!({"error": error.to_string()})),
        }
    }

    async fn put_host_health(&self, request: &Request, query: &str) -> Response {
        match authorize_host_health(self, request).await {
            Ok(true) => {}
            Ok(false) => return send_json(http_status("401"), &json!({"error": "unauthorized"})),
            // An unreadable authorization item is this service's failure, not
            // the caller's credential. Answering 401 for it told every host in
            // the fleet its beacon grant had been rejected while the real
            // fault was local and retryable, and the beacons stayed silent
            // for seventeen hours behind that sentence.
            Err(()) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "host-health authorization unavailable"}),
                )
            }
        }
        let values = parse_qs(query);
        let host = match values.as_slice() {
            [(key, value)] if key == "host" => value.clone(),
            _ => {
                return send_json(
                    http_status("400"),
                    &json!({"error": "exactly one host query parameter is required"}),
                )
            }
        };
        if !valid_beacon_host(&host) {
            return send_json(
                http_status("400"),
                &json!({"error": "host must be a lowercase DNS label"}),
            );
        }
        let content_type = request
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let content_length = request
            .header("content-length")
            .and_then(|value| value.parse::<usize>().ok());
        if content_type != "application/json"
            || content_length != Some(request.body.len())
            || request.body.is_empty()
        {
            return send_json(
                http_status("400"),
                &json!({"error": "invalid JSON request framing"}),
            );
        }
        let payload: Value = match serde_json::from_slice(&request.body) {
            Ok(value) => value,
            Err(error) => {
                return send_json(
                    http_status("400"),
                    &json!({"error": format!("invalid JSON: {error}")}),
                )
            }
        };
        let Some(document) = payload.as_object() else {
            return send_json(
                http_status("400"),
                &json!({"error": "host beacon must be a JSON object"}),
            );
        };
        if document.get("host").and_then(Value::as_str) != Some(host.as_str())
            || document
                .get("reported_at")
                .and_then(Value::as_str)
                .is_none()
            || document.get("units").and_then(Value::as_object).is_none()
        {
            return send_json(
                http_status("400"),
                &json!({"error": "beacon host must match the query and reported_at/units are required"}),
            );
        }
        let path = crate::monitor::host_health::beacon_object_path(&host);
        match self.store.upload_bytes(&path, &request.body).await {
            Ok(()) => send_json(
                http_status("200"),
                &json!({"state": "stored", "host": host, "path": path}),
            ),
            Err(error) => send_json(http_status("500"), &json!({"error": error.to_string()})),
        }
    }

    fn deployment_id(&self) -> String {
        config::stado_deployment_id()
    }

    fn trusted_request_host(&self, value: Option<&str>, forwarded_proto: Option<&str>) -> bool {
        let reverse_proxy_enabled =
            config::dashboard_trust_https_proxy() || !self.deployment_id().is_empty();
        trusted_request_host(value, forwarded_proto, reverse_proxy_enabled)
    }
}

/// Python `serve(host=None, port=None)`: run the API listener. Blocks until
/// killed. Defaults from `config::dashboard_bind()` /
/// `config::dashboard_port()`; storage from `config::bucket()`.
///
/// `enrollment_only` narrows the listener to `ENROLLMENT_ROUTES` — the mode
/// that is safe to publish through a tunnel.
pub async fn serve(
    host: Option<&str>,
    port: Option<i64>,
    enrollment_only: bool,
) -> Result<(), DashboardError> {
    let host = host
        .map(str::to_string)
        .unwrap_or_else(|| config::dashboard_bind().to_string());
    let port = port.unwrap_or_else(config::dashboard_port);
    let port = u16::try_from(port)
        .map_err(|_| DashboardError::Other(format!("dashboard port out of range: {port}")))?;
    let store = JobStorage::for_server().await?;
    Dashboard::new(store)
        .with_enrollment_only(enrollment_only)
        .serve_with(&host, port)
        .await
}

// ---------------------------------------------------------------------------
// Minimal hand-rolled HTTP/1.1 (no framework dependency, per the port spec)
// ---------------------------------------------------------------------------

/// Request head cap (Python's http.server parses a similar 64 KiB budget).
const MAX_HEAD_BYTES: usize = 65536;

static MACHINE_JOB_ID_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
        .expect("static machine job ID regex compiles")
});

struct Request {
    method: String,
    /// Raw request target including the query string (Python `self.path`).
    path: String,
    /// Lowercased names with trimmed values.
    headers: Vec<(String, String)>,
    content_length: usize,
    body: Vec<u8>,
    /// Connection peer, filled in by the accept path. `None` when the socket
    /// no longer has one to report.
    peer: Option<std::net::IpAddr>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Read one request with a bounded head and body. Body framing is deliberately
/// limited to Content-Length; mutating routes reject Transfer-Encoding.
async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete HTTP request head",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP request head too large",
            ));
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let (method, path) = match (parts.next(), parts.next(), parts.next()) {
        (Some(method), Some(path), Some(version)) if version.starts_with("HTTP/") => {
            (method.to_string(), path.to_string())
        }
        _ => (String::new(), String::new()),
    };
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            if name == "transfer-encoding" {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Transfer-Encoding is unsupported",
                ));
            }
            if name == "content-length" && headers.iter().any(|(key, _)| key == &name) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate Content-Length",
                ));
            }
            headers.push((name, value.trim().to_string()));
        }
    }
    let body_start = head_end + b"\r\n\r\n".len();
    let content_length_header = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.as_str());
    let object_put = method == "PUT" && path.starts_with("/api/object?");
    let content_length = match content_length_header {
        Some(value) => value.parse::<usize>().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid Content-Length")
        })?,
        None if object_put => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "object PUT requires Content-Length",
            ));
        }
        None => usize::default(),
    };
    let max_body_bytes = if object_put {
        crate::object_store::max_object_bytes()
    } else {
        MAX_HEAD_BYTES
    };
    if content_length > max_body_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP request body too large",
        ));
    }
    let available = buf.len().saturating_sub(body_start).min(content_length);
    let mut body = Vec::with_capacity(content_length);
    body.extend_from_slice(&buf[body_start..body_start + available]);
    if body.len() < content_length {
        let received = body.len();
        body.resize(content_length, 0);
        stream.read_exact(&mut body[received..]).await?;
    }
    Ok(Some(Request {
        method,
        path,
        headers,
        content_length,
        body,
        peer: None,
    }))
}

struct Response {
    status: u16,
    bytes: Vec<u8>,
}

impl Response {
    fn new(status: u16, reason: &str, content_type: &str, body: &[u8]) -> Self {
        Self::new_with_headers(status, reason, content_type, body, &[])
    }

    fn new_with_headers(
        status: u16,
        reason: &str,
        content_type: &str,
        body: &[u8],
        headers: &[(&str, String)],
    ) -> Self {
        let mut head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("Connection: close\r\n\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body);
        Self { status, bytes }
    }

    fn json(status: u16, body: &str) -> Self {
        let reason = match status {
            200 => "OK",
            401 => "Unauthorized",
            409 => "Conflict",
            _ => "OK",
        };
        Self::new(status, reason, "application/json", body.as_bytes())
    }

    fn text(status: u16, reason: &str, body: &str) -> Self {
        Self::new(status, reason, "text/plain; charset=utf-8", body.as_bytes())
    }
}

fn parse_byte_range(value: &str, length: usize) -> Option<(usize, usize)> {
    if length == usize::default() {
        return None;
    }
    let value = value.strip_prefix("bytes=")?;
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    let start = start.parse::<usize>().ok()?;
    if start >= length {
        return None;
    }
    let last = length.saturating_sub(usize::from(true));
    let end = if end.is_empty() {
        last
    } else {
        end.parse::<usize>().ok()?.min(last)
    };
    (start <= end).then_some((start, end))
}

fn http_status(value: &str) -> u16 {
    value.parse().expect("static HTTP status is valid")
}

fn empty_response(status: u16, reason: &str) -> Response {
    Response::new(status, reason, "text/plain; charset=utf-8", b"")
}

/// Python `_Handler._send_json`:
/// `json.dumps(payload, default=str, sort_keys=True, separators=(",", ":"))`.
fn send_json(status: u16, payload: &Value) -> Response {
    Response::json(status, &json_dumps_sorted_compact(payload))
}

fn machine_result_response(result: Result<Value, MachineError>) -> Response {
    match result {
        Ok(result) => send_json(
            http_status("200"),
            &json!({"schema_version": MACHINE_SCHEMA_VERSION, "ok": true, "result": result}),
        ),
        Err(error) => {
            let status = match error.code.as_str() {
                "INVALID_REQUEST" | "INVALID_SOURCE_ARCHIVE" => http_status("400"),
                "NOT_FOUND" => http_status("404"),
                "IDEMPOTENCY_CONFLICT" => http_status("409"),
                "UNAUTHORIZED" => http_status("401"),
                "FORBIDDEN" => http_status("403"),
                _ if error.retryable => http_status("503"),
                _ => http_status("500"),
            };
            send_json(
                status,
                &json!({
                    "schema_version": MACHINE_SCHEMA_VERSION,
                    "ok": false,
                    "error": {
                        "code": error.code,
                        "message": error.message,
                        "retryable": error.retryable,
                    },
                }),
            )
        }
    }
}

fn invalid_machine_request(message: impl Into<String>) -> Response {
    machine_result_response(Err(MachineError::new("INVALID_REQUEST", message)))
}

fn machine_job_id(query: &str) -> Result<&str, Response> {
    let invalid =
        || invalid_machine_request("query must contain exactly one path-safe job_id parameter");
    if query.is_empty() || query.contains('&') {
        return Err(invalid());
    }
    let Some((name, job_id)) = query.split_once('=') else {
        return Err(invalid());
    };
    if name != "job_id" || !MACHINE_JOB_ID_RE.is_match(job_id) {
        return Err(invalid());
    }
    Ok(job_id)
}

fn service_name(query: &str) -> Result<&str, Response> {
    let invalid = || {
        invalid_service_request(
            "query must contain exactly one lowercase, path-safe name parameter",
        )
    };
    if query.is_empty() || query.contains('&') {
        return Err(invalid());
    }
    let Some((key, name)) = query.split_once('=') else {
        return Err(invalid());
    };
    if key != "name" || service::validate_service_name(name).is_err() {
        return Err(invalid());
    }
    Ok(name)
}

async fn service_beacon_store() -> Result<JobStorage, String> {
    let bucket = targets::GCS_REGISTRY_URI
        .split_once("//")
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default())
        .unwrap_or_default();
    JobStorage::with_bucket(bucket)
        .await
        .map_err(|error| error.to_string())
}

async fn declared_services_matching(name: &str) -> Result<Vec<service::ManagedService>, String> {
    let registry = targets::fetch_registry_remote()
        .await
        .map_err(|error| error.to_string())?;
    let mut found = Vec::new();
    for target in registry.local_targets() {
        found.extend(
            service::declared_services(target)
                .into_iter()
                .filter(|declared| declared.matches(name)),
        );
    }
    Ok(found)
}

fn service_success(result: Value) -> Response {
    send_json(http_status("200"), &json!({"ok": true, "result": result}))
}

fn service_failure(
    status: u16,
    code: &str,
    message: impl Into<String>,
    retryable: bool,
) -> Response {
    send_json(
        status,
        &json!({
            "ok": false,
            "error": {
                "code": code,
                "message": message.into(),
                "retryable": retryable,
            },
        }),
    )
}

fn invalid_service_request(message: impl Into<String>) -> Response {
    service_failure(http_status("400"), "INVALID_REQUEST", message, false)
}

fn validate_remote_machine_request(request: &Value) -> Result<(), MachineError> {
    let Some(request) = request.as_object() else {
        return Ok(());
    };
    if request.contains_key("source_archive_path") {
        return Err(MachineError::new(
            "INVALID_REQUEST",
            "source_archive_path is not accepted by the remote machine API; upload through the object API and declare a stado:// input_object",
        ));
    }
    let Some(inputs) = request.get("input_objects").and_then(Value::as_object) else {
        return Ok(());
    };
    for value in inputs.values() {
        let Some(spec) = value.as_object() else {
            continue;
        };
        if spec
            .keys()
            .any(|key| !matches!(key.as_str(), "stado_uri" | "relative_path" | "sha256"))
        {
            return Err(MachineError::new(
                "INVALID_REQUEST",
                "input_objects entries accept only stado_uri, relative_path, and sha256",
            ));
        }
    }
    Ok(())
}

/// Python `parse_qs(urlsplit(self.path).query)`: `&`-separated `key=value`
/// pairs, percent-decoded with `+` as space.
fn parse_qs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (url_decode(key), url_decode(value))
        })
        .collect()
}

fn query_value(values: &[(String, String)], name: &str) -> Option<String> {
    values
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn object_from_query(query: &str) -> Result<crate::object_store::ObjectRef, Response> {
    let values = parse_qs(query);
    let uri = query_value(&values, "uri").unwrap_or_default();
    if uri.is_empty() {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "uri is required"}),
        ));
    }
    crate::object_store::ObjectRef::parse(&uri)
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))
}

fn object_list_from_query(query: &str) -> Result<(String, String), Response> {
    let values = parse_qs(query);
    let raw_namespace = query_value(&values, "namespace").unwrap_or_default();
    if raw_namespace.is_empty() {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "namespace is required"}),
        ));
    }
    let sentinel = crate::object_store::ObjectRef::new(&raw_namespace, "sentinel")
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))?;
    let namespace = sentinel.namespace().to_string();
    let prefix = query_value(&values, "prefix")
        .unwrap_or_default()
        .trim_matches('/')
        .to_string();
    crate::object_store::ObjectRef::namespace_prefix(&namespace, &prefix)
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))?;
    Ok((namespace, prefix))
}

fn merged_object_metadata(
    object: &ObjectRef,
    content_type: &str,
    extra: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, &'static str> {
    let mut metadata = crate::object_store::metadata(object, content_type);
    for (name, value) in extra {
        if !name.starts_with("stado-")
            || metadata.contains_key(name)
            || value.is_empty()
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err("custom object metadata must use unique non-empty stado-* fields");
        }
        metadata.insert(name.clone(), value.clone());
    }
    Ok(metadata)
}

fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut digest = [0u8; 32];
    hex::decode_to_slice(value, &mut digest).ok()?;
    Some(digest)
}

/// Composition has a transport envelope because the client has already
/// uploaded every chunk before this request. A failed composition must carry
/// its exact retriable status without an intermediary replacing the JSON body.
fn object_compose_response(status: u16, payload: Value) -> Response {
    send_json(
        http_status("200"),
        &json!({"status": status, "payload": payload}),
    )
}

fn object_compose_error(status: u16, message: impl Into<String>) -> Response {
    object_compose_response(status, json!({"error": message.into()}))
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (
                    bytes.get(i + 1).and_then(|b| hex(*b)),
                    bytes.get(i + 2).and_then(|b| hex(*b)),
                ) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi * 16 + lo) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Host-header DNS-rebinding guard
// ---------------------------------------------------------------------------

/// Accept loopback Host values for direct local access. A configured HTTPS
/// reverse proxy may forward either DNS or IP Host values; because the listener
/// itself is loopback-only, `X-Forwarded-Proto` cannot be supplied by external
/// plaintext ingress. Dashboard authorization remains a separate boundary.
fn trusted_request_host(
    value: Option<&str>,
    forwarded_proto: Option<&str>,
    reverse_proxy_enabled: bool,
) -> bool {
    let Some(value) = value else { return false };
    if value.is_empty() {
        return false;
    }
    // Malformed authorities always fail closed.
    let proxy_https = reverse_proxy_enabled && forwarded_proto == Some("https");

    // authority = [userinfo@]host[:port]; path/query/fragment split off.
    let (authority, has_suffix) = match value.find(['/', '?', '#']) {
        Some(index) => (&value[..index], true),
        None => (value, false),
    };
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, authority),
    };
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, after)) => {
                let port = if after.is_empty() {
                    None
                } else if let Some(port) = after.strip_prefix(':') {
                    Some(port)
                } else {
                    // "[::1]junk" — Python raises ValueError on .hostname.
                    return false;
                };
                (host, port)
            }
            // Unterminated bracket — urlsplit raises ValueError.
            None => return false,
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    // Python `_ = parsed.port`: an unparseable/out-of-range port raises
    // ValueError -> the DNS branch (even for IP hosts).
    if let Some(port) = port {
        if port.parse::<u16>().is_err() {
            return false;
        }
    }
    // Python: `if not host or parsed.username or parsed.password or
    // parsed.path or parsed.query or parsed.fragment: return False`.
    let (username, password) = match userinfo {
        Some(userinfo) => match userinfo.split_once(':') {
            Some((name, password)) => (name, Some(password)),
            None => (userinfo, None),
        },
        None => ("", None),
    };
    if host.is_empty()
        || !username.is_empty()
        || password.is_some_and(|password| !password.is_empty())
        || has_suffix
    {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback() || proxy_https,
        Err(_) => proxy_https,
    }
}

fn valid_beacon_host(host: &str) -> bool {
    let bytes = host.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let left = Sha256::digest(left);
    let right = Sha256::digest(right);
    let mut difference = u8::default();
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == u8::default()
}

/// Accept a bearer only after the request has resolved to one canonical
/// namespace and key boundary. Out-of-scope requests and bearer mismatches are
/// unauthorized; invalid configuration or an unavailable exact item is
/// reported separately so the route can return a redacted 503.
fn release_object_namespace(namespace: &str) -> bool {
    matches!(namespace, "releases" | "sources")
}

/// Return the immutable target governed by one disposable chunk key.
///
/// Release objects remain undeletable. Only the exact staging shape emitted by
/// the chunked uploader can be removed, and it is authenticated against the
/// final target rather than against an independently chosen prefix.
fn release_upload_target_key(key: &str) -> Option<&str> {
    let (target, suffix) = key.split_once(".__stado_upload/")?;
    let mut parts = suffix.split('/');
    let upload_id = parts.next()?;
    let chunk_index = parts.next()?;
    if target.is_empty()
        || upload_id.len() != 64
        || !upload_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || chunk_index.len() != 8
        || !chunk_index.bytes().all(|byte| byte.is_ascii_digit())
        || parts.next().is_some()
    {
        return None;
    }
    Some(target)
}

/// Route-scoped host beacon publication: the bearer stored as
/// `stado-host-health-api/token` and nothing else. The dashboard resolves it
/// through the same dedicated verifier grant as its object routes, never
/// through the broad coordinator credential. Machine publishers are
/// authorized separately through their exact client policies.
///
/// `Ok(false)` is a rejected bearer. `Err(())` is this service being unable to
/// read the item it compares against — a local, retryable fault that the
/// caller cannot fix by presenting a different credential.
async fn authorize_host_health(dashboard: &Dashboard, request: &Request) -> Result<bool, ()> {
    let expected = dashboard
        .object_token("host-health", crate::config::HOST_HEALTH_API_ITEM)
        .await?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

async fn authorize_object(
    dashboard: &Dashboard,
    request: &Request,
    namespace: &str,
    key_or_prefix: &str,
    list: bool,
    action: &str,
) -> Result<bool, ()> {
    let namespaces = config::object_api_namespaces().map_err(|_| ())?;
    let Some(policy) = namespaces.get(namespace) else {
        return Ok(false);
    };
    let in_scope = if list {
        policy
            .authorized_list_prefix(key_or_prefix, action)
            .is_some()
    } else {
        policy.allows_object_action(key_or_prefix, action)
    };
    if !in_scope {
        return Ok(false);
    }
    let expected = dashboard.object_token(namespace, policy.item()).await?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

/// Authenticate one immutable release publisher after resolving the exact
/// product prefix inside `stado://releases`. The former global object token is
/// never consulted.
async fn authorize_release(
    dashboard: &Dashboard,
    request: &Request,
    key_or_prefix: &str,
    list: bool,
) -> Result<bool, ()> {
    config::release_api_publishers().map_err(|_| ())?;
    let policy = if list {
        config::release_publisher_for_list(key_or_prefix).map(|(policy, _)| policy)
    } else {
        config::release_publisher_for_key(key_or_prefix)
    };
    let Some(policy) = policy else {
        return Ok(false);
    };
    let expected = dashboard.release_token(policy.item()).await?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

async fn authorize_service(request: &Request, service: &str, action: &str) -> Result<bool, ()> {
    config::service_api_deployers().map_err(|_| ())?;
    let Some(policy) = config::service_deployer_for(service, action) else {
        return Ok(false);
    };
    let expected = crate::skarbiec::read_service_token(policy.item(), "token")
        .await
        .map_err(|_| ())?
        .filter(|value| !value.is_empty())
        .ok_or(())?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

fn machine_result_target(value: &Value) -> Option<&str> {
    value
        .get("job")
        .and_then(Value::as_object)
        .and_then(|job| job.get("provider"))
        .and_then(Value::as_str)
        .filter(|target| !target.is_empty())
}

async fn authenticate_machine_client(
    request: &Request,
    action: &str,
) -> Result<Option<&'static config::MachineApiClient>, ()> {
    let Some(supplied) = request
        .header("authorization")
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let clients = config::machine_api_clients().map_err(|_| ())?;
    let mut matched = None;
    for client in clients
        .values()
        .filter(|client| client.allows_action(action))
    {
        let expected = crate::skarbiec::read_machine_token(client.item(), "token")
            .await
            .map_err(|_| ())?
            .filter(|value| !value.is_empty())
            .ok_or(())?;
        if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            if matched.is_some() {
                return Ok(None);
            }
            matched = Some(client);
        }
    }
    Ok(matched)
}
