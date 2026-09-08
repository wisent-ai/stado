//! Which boundaries one request enforces and which it revalidates, and the
//! proof that every boundary has a request that can reopen it.

use super::Boundary;

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
pub(crate) struct BoundaryPlan {
    /// Boundaries this request may not proceed without. A closed one answers
    /// `503` and the request stops.
    pub(crate) enforced: Vec<Boundary>,
    /// Boundaries this request revalidates, whether or not it is gated by
    /// them. Always a superset of `enforced`: enforcing without asking is what
    /// froze `Release`, and asking without enforcing is what unfreezes it.
    pub(crate) revalidated: Vec<Boundary>,
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
pub(crate) fn boundary_plan(path: &str, object: Option<(&str, &str)>) -> BoundaryPlan {
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
        "/api/host/inventory"
        | "/api/host/storage-root-reconcile"
        | "/api/service/converge"
        | "/api/registry.json"
        | "/api/registry/policy"
        | "/api/cleanup.json"
        | "/api/cleanup/run" => BoundaryPlan::gated(&[Boundary::Registry]),
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
pub(crate) fn requires_object_boundary(namespace: &str, key: &str) -> bool {
    crate::object_store::release_policy_key(namespace, key).is_none()
}

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
    ("/api/service/converge", None),
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
