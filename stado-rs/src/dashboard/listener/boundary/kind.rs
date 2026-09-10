//! The boundary vocabulary: which boundaries exist, what requires each one,
//! and the key and label every served document and log line uses for it.

/// One authorization boundary this listener gates its routes on.
///
/// An enum rather than seven named booleans because the recovery path needs
/// to name a boundary as a value: claim its cooldown, run exactly its
/// verifier, record exactly its verdict. Seven fields could only be reached
/// by seven copies of that sequence, which is how the startup block came to
/// hold seven near-identical macro expansions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Boundary {
    Object,
    Release,
    Machine,
    Service,
    RateLimitVerifier,
    RateLimitState,
    Integration,
    /// The operator-owned registry and host-control routes the desktop app
    /// calls.
    ///
    /// Its verifier is ready even when nothing is declared: an undeclared
    /// boundary refuses every request with `401`, which is what "nobody has
    /// been granted this" means, and reporting it as unavailable would send an
    /// operator looking for a broken vault.
    Registry,
}

impl Boundary {
    /// Every boundary, in the deterministic order startup validates them.
    /// Also the order the served documents list them in, so an operator
    /// comparing a health document with a startup log reads one sequence.
    pub(crate) const ALL: [Boundary; 8] = [
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
    /// - `Registry` — the registry-policy, cleanup, host inventory, and
    ///   full-host service convergence routes.
    ///
    /// A boundary that answers this question with "nothing" must not be
    /// reported: an operator reading `/healthz` has to be able to conclude
    /// something true from every field in it.
    pub(crate) fn required_by(self) -> &'static str {
        match self {
            Boundary::Object => "/api/object, /api/object/list, /api/object/stat",
            Boundary::Release => "the object routes for a release coordinate",
            Boundary::Machine => "/api/machine/status, /api/machine/submit, /api/machine/cancel",
            Boundary::Service => "/api/service/status, /api/service/restart",
            Boundary::RateLimitVerifier | Boundary::RateLimitState => "/api/rate-limit/consume",
            Boundary::Integration => "the integration routes",
            Boundary::Registry => {
                "/api/registry.json, /api/registry/policy, /api/registry/import, \
                 /api/cleanup.json, /api/cleanup/run, /api/memory-policies.json, \
                 /api/host/inventory, /api/service/converge, \
                 /api/host/storage-root-reconcile"
            }
        }
    }

    /// The key this boundary carries in `/healthz`.
    /// Unchanged from the flat booleans `/healthz` has always served.
    pub(crate) fn key(self) -> &'static str {
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
    pub(crate) fn label(self) -> &'static str {
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
