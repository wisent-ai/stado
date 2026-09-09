//! No boundary this process found closed stays frozen at its boot verdict.
//!
//! A boundary revalidates only when a request asks about it, so a boundary
//! that appears in no request's revalidation set is frozen for the life of the
//! process — closed forever, whichever way it started. That is what
//! `Boundary::Release` was, twice, and it was invisible because the rule was
//! only ever checked by reading the routing table.
//!
//! This case answers it from outside: with every verifier's endpoint dead,
//! every boundary the listener reports closed must get a fresh verdict from a
//! request an operator can send, and the ordering the product's own budget
//! imposes — one revalidation per request, the first closed boundary in the
//! plan — is asserted rather than assumed, because it is why a second-place
//! boundary like `release` needs its predecessor open before it can reopen at
//! all.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::fixture::{Env, COOLDOWN_SECONDS};
use crate::policy::{ABSENT_KEY, NAMESPACE, UNDECLARED_RELEASE_KEY};

/// One request per route family the listener's boundary plan knows, each the
/// request an operator's client really sends for that surface.
const PROBES: &[(&str, &str)] = &[
    ("GET", "/api/object?uri=stado://PROBE_NAMESPACE/PROBE_KEY"),
    ("GET", "/api/object?uri=stado://releases/PROBE_RELEASE"),
    ("GET", "/api/object/list?namespace=sources&prefix=stado/"),
    ("POST", "/api/rate-limit/consume"),
    ("GET", "/api/machine/status"),
    ("GET", "/api/service/status"),
    ("GET", "/api/integration/anything"),
    ("GET", "/api/service/converge"),
];

/// The boundaries a request can revalidate directly, because each is first in
/// some route's plan. `release` and `rate_limit_state` are deliberately
/// absent: each sits behind another boundary in its own plan, which is the
/// ordering this case also asserts.
///
/// `registry` is absent for a different reason and by the product's own
/// design: its verifier is ready even when nothing is declared, because an
/// undeclared registry boundary refuses every request with `401` and
/// reporting it unavailable would send an operator looking for a broken
/// vault. A boundary that never closes here has no frozen verdict to prove.
const DIRECTLY_REACHABLE: &[&str] = &[
    "object",
    "machine",
    "service",
    "rate_limit_verifier",
    "integration",
];

/// The boundaries no single request can reach while their predecessor is
/// closed: each is second in the only plan that names it, and a request
/// revalidates the first closed boundary it finds and no more. `release`
/// reopens through an open `object` — the case next door proves that — and
/// `rate_limit_state` behind `rate_limit_verifier`.
const BEHIND_ANOTHER: &[&str] = &["release", "rate_limit_state"];

fn target(template: &str) -> String {
    template
        .replace("PROBE_NAMESPACE", NAMESPACE)
        .replace("PROBE_KEY", ABSENT_KEY)
        .replace("PROBE_RELEASE", UNDECLARED_RELEASE_KEY)
}

/// Every boundary's `checked_at`, which is the operator-visible answer to
/// "when was this verdict last reached".
fn verdict_stamps(state: &Value) -> BTreeMap<String, String> {
    state["boundaries"]
        .as_object()
        .expect("the state document lists boundaries")
        .iter()
        .map(|(key, boundary)| {
            (
                key.clone(),
                boundary["checked_at"]
                    .as_str()
                    .expect("every boundary carries a timestamp")
                    .to_string(),
            )
        })
        .collect()
}

fn closed(state: &Value) -> Vec<String> {
    state["boundaries"]
        .as_object()
        .expect("the state document lists boundaries")
        .iter()
        .filter(|(_, boundary)| boundary["ready"] == Value::Bool(false))
        .map(|(key, _)| key.clone())
        .collect()
}

#[test]
fn no_boundary_the_listener_found_closed_stays_frozen_at_its_boot_verdict() {
    let env = Env::new();
    // Nothing serves either endpoint: every verifier fails, so the listener
    // boots with the shut boundaries this case is about.
    let dead = format!("http://127.0.0.1:{}", crate::vault::reserved_port());
    let listener = env.start(&dead, &dead);

    let boot = listener.state();
    let boot_stamps = verdict_stamps(&boot);
    // Every boundary the listener itself reports closed, minus the two that
    // sit behind another boundary in their own plan. Read off the served
    // document rather than listed here, so a boundary added later without a
    // route that can reopen it fails this case instead of slipping past a
    // list nobody updated.
    let required: Vec<String> = closed(&boot)
        .into_iter()
        .filter(|boundary| !BEHIND_ANOTHER.contains(&boundary.as_str()))
        .collect();
    for boundary in DIRECTLY_REACHABLE {
        assert!(
            required.contains(&(*boundary).to_string()),
            "this case is only about closed boundaries, and {boundary} did not close: {boot}\n{}",
            listener.logged()
        );
    }

    // Drive the operator-reachable request for each route family until every
    // closed boundary has been given a fresh verdict. Each request revalidates
    // at most one boundary and only once per cooldown, so this takes several
    // passes by design.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut refreshed: BTreeMap<String, String> = BTreeMap::new();
    loop {
        for (method, template) in PROBES {
            let target = target(template);
            let answer = match *method {
                "POST" => listener.post(&target, "{}"),
                _ => listener.get(&target, None),
            };
            assert!(
                answer.status >= 400,
                "a closed boundary must refuse rather than serve {target}: {} {}",
                answer.status,
                answer.body
            );
        }
        let stamps = verdict_stamps(&listener.state());
        for (boundary, stamp) in stamps {
            if boot_stamps.get(&boundary) != Some(&stamp) {
                refreshed.insert(boundary, stamp);
            }
        }
        let unreached: Vec<&String> = required
            .iter()
            .filter(|boundary| !refreshed.contains_key(*boundary))
            .collect();
        if unreached.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no request an operator can send revalidates {unreached:?}, so those boundaries are \
             frozen at their boot verdict for the life of the process\n{}",
            listener.logged()
        );
        std::thread::sleep(Duration::from_secs(COOLDOWN_SECONDS));
    }

    // The ordering that makes `release` need an open `object` first: while the
    // object boundary is closed, the release-coordinate route spends its one
    // revalidation on `object` and never reaches `release`.
    assert!(
        !refreshed.contains_key("release"),
        "a request revalidates one boundary, the first closed one in its plan; release must wait \
         for object: {refreshed:?}"
    );
}
