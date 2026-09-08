//! Serving the machine-side bootstrap script this binary embeds.

use crate::dashboard::{http_status, Response};

use super::super::refusals::unavailable;
use super::super::JOIN_SCRIPT;

/// `GET /join.sh` — the machine-side bootstrap script, verbatim from the
/// repository, unauthenticated. The script discloses nothing: the code is the
/// user's own argument to it. Never cached, so a re-issued script reaches the
/// next machine that runs the line.
pub(in crate::dashboard) fn join_script() -> Response {
    if JOIN_SCRIPT.is_empty() {
        return unavailable(
            "join script unavailable: this build has no deploy/join.sh in its source tree",
        );
    }
    Response::new_with_headers(
        http_status("200"),
        "OK",
        "text/plain; charset=utf-8",
        JOIN_SCRIPT.as_bytes(),
        &[
            (
                "Cache-Control",
                "no-store, no-cache, must-revalidate".to_string(),
            ),
            ("Pragma", "no-cache".to_string()),
        ],
    )
}
