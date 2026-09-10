//! Who may ask, and what a malformed ask answers with: a missing bearer, a
//! wrong bearer, a bearer without the action, and every shape of request the
//! Services API refuses before it reaches a host.

use serde_json::json;

use crate::fixture::{DashboardFixture, HOST};
use crate::helpers::{assert_error, digest};

/// None of these reaches a host, and none of them may touch a byte on disk.
pub(crate) fn refuse_unauthorized_and_malformed(
    fixture: &DashboardFixture,
    selected: &str,
    read_bearer: &str,
    apply_bearer: &str,
    protected_baseline: &str,
    stado_baseline: &str,
    skarbiec_baseline: &str,
) {
    let no_bearer = fixture.request("GET", &selected, None, "");
    assert_eq!(no_bearer.status, 401, "missing bearer: {}", no_bearer.body);
    assert_eq!(no_bearer.body, json!({"error": "unauthorized"}));
    let wrong_bearer = fixture.request("GET", &selected, Some("not-a-real-grant"), "");
    assert_eq!(
        wrong_bearer.status, 401,
        "wrong bearer: {}",
        wrong_bearer.body
    );
    assert_eq!(wrong_bearer.body, json!({"error": "unauthorized"}));

    let apply_cannot_read = fixture.request("GET", &selected, Some(&apply_bearer), "");
    assert_eq!(
        apply_cannot_read.status, 401,
        "apply-only grant read the route: {}",
        apply_cannot_read.body
    );
    let read_cannot_apply = fixture.request("POST", &selected, Some(&read_bearer), "");
    assert_eq!(
        read_cannot_apply.status, 401,
        "read-only grant applied convergence: {}",
        read_cannot_apply.body
    );

    for malformed in [
        "/api/service/converge",
        "/api/service/converge?binary=stado",
        "/api/service/converge?target=",
        "/api/service/converge?target=one&binary=",
        "/api/service/converge?target=one&target=two",
        "/api/service/converge?target=one&binary=stado&binary=skarbiec",
        "/api/service/converge?target=one&unexpected=value",
        "/api/service/converge?target=%GG",
        "/api/service/converge?target=%FF",
    ] {
        assert_error(
            &fixture.request("GET", malformed, Some(&read_bearer), ""),
            400,
            "INVALID_REQUEST",
        );
    }
    assert_error(
        &fixture.request("POST", &selected, Some(&apply_bearer), "{}"),
        400,
        "INVALID_REQUEST",
    );
    assert_error(
        &fixture.request(
            "GET",
            "/api/service/converge?target=no-such-declared-host",
            Some(&read_bearer),
            "",
        ),
        503,
        "SERVICE_CONVERGE_FAILED",
    );
    assert_error(
        &fixture.request(
            "GET",
            &format!("/api/service/converge?target={HOST}&binary=no-such-binary"),
            Some(&read_bearer),
            "",
        ),
        503,
        "SERVICE_CONVERGE_FAILED",
    );
    assert_eq!(digest(&fixture.protected), protected_baseline);
    assert_eq!(digest(&fixture.stado), stado_baseline);
    assert_eq!(digest(&fixture.skarbiec), skarbiec_baseline);
}
