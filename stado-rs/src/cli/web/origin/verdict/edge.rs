//! Which origin the live public edge says it selected, and the row for one it
//! selected that nothing declares.
//!
//! Split out of the verdict itself because it is the only one of the three
//! readings that asks the deployment about its own configuration rather than
//! asking the world about a name.

use serde_json::{json, Value};
use crate::deploy::host_gates::{observe, DiagnosticRead};
use crate::deploy::DeployError;

use crate::public_origin::{self, PublicOrigin, ResolutionState};

/// Where the public edge reports the origin it selected. The path is the
/// release route's sibling: one route serves bytes, this one serves the
/// selection, and the second exists because the first only named its origin
/// while failing.
const SELECTION_PATH: &str = "/api/release/origin";

/// How much of an unexpected response body a receipt carries.
///
/// The answer to this request is a small JSON document. A deployment that
/// predates the selection route answers with the site's own 404 page instead,
/// and pasting a Next.js document into a receipt buries the one sentence an
/// operator needs under sixty kilobytes of script tags.
const BODY_EXCERPT_BYTES: usize = 200;

/// What the live public edge says it selected.
pub(crate) struct EdgeSelection {
    pub endpoint: String,
    pub origin: Option<String>,
    pub detail: String,
    pub diagnosis: Value,
    pub observation: DiagnosticRead,
}

impl EdgeSelection {
    pub fn readback_answered(&self) -> bool {
        // The deployment probes the real object route without a URI; its
        // documented 400 is an answered request, not an upstream failure.
        self.diagnosis["probe"].get("error").is_none_or(Value::is_null)
            && self.diagnosis["probe"]["status"]
                .as_u64()
                .is_some_and(|status| (200..300).contains(&status) || status == 400)
    }

    pub fn readback_detail(&self) -> &str {
        self.diagnosis["probe"]["detail"]
            .as_str()
            .unwrap_or("the public edge supplied no origin read-back evidence")
    }

    pub fn report(&self, state: &str) -> Value {
        json!({
            "state": state, "origin": self.origin, "endpoint": self.endpoint,
            "detail": self.detail, "diagnosis": self.diagnosis,
            "observation": self.observation,
        })
    }
}

pub(crate) async fn edge_selection() -> EdgeSelection {
    let endpoint = format!(
        "{}{SELECTION_PATH}",
        crate::config::stado_api_url().trim_end_matches('/')
    );
    let (answer, observation) = observe(
        "edge_selection", endpoint.clone(), read_selection(&endpoint),
    ).await;
    let (origin, detail, diagnosis) = match answer {
        Some(answer) => answer,
        None => (None, observation.detail.clone().unwrap_or_default(), Value::Null),
    };
    EdgeSelection { endpoint, origin, detail, diagnosis, observation }
}

async fn read_selection(endpoint: &str) -> Result<(Option<String>, String, Value), DeployError> {
    let client = crate::cli::storage::fleet_https_client()
        .map_err(|error| DeployError(format!("could not build HTTPS client: {error}")))?;
    let response = client.get(endpoint).send().await
        .map_err(|error| DeployError(format!("public edge request failed: {error:?}")))?;
    let status = response.status().as_u16();
    let body = response.text().await
        .map_err(|error| DeployError(format!("public edge response body failed: {error:?}")))?;
    let payload: Value = serde_json::from_str(&body).map_err(|error| DeployError(format!(
        "public edge answered HTTP {status} with invalid JSON: {error}; {}", quoted_body(&body)
    )))?;
    if !(200..300).contains(&status) {
        return Err(DeployError(format!("public edge answered HTTP {status}: {}", quoted_body(&body))));
    }
    let origin = payload["origin"].as_str().map(str::to_string);
    let detail = match &origin {
        Some(origin) => format!("the public edge reports it fetches release objects from {origin}"),
        None => format!("the public edge answered HTTP {status} and named no selected origin"),
    };
    let diagnosis = payload.get("originDiagnosis").cloned().unwrap_or(Value::Null);
    Ok((origin, detail, diagnosis))
}

fn quoted_body(body: &str) -> String {
    let body = body.trim();
    let head: String = body.chars().take(BODY_EXCERPT_BYTES).collect();
    if head.len() == body.len() {
        return head;
    }
    format!("{head}… ({} bytes in total, excerpted)", body.len())
}

pub(crate) fn edge_state(origin: &PublicOrigin, selection: &EdgeSelection) -> &'static str {
    match &selection.origin {
        Some(selected) if *selected == origin.origin() => "agrees",
        Some(_) => "differs",
        None => "unreadable",
    }
}

/// The row for a public origin no declaration covers.
///
/// Two shapes reach it. The edge named an origin nothing declares, which is
/// the 2026-09-07 defect exactly. Or the edge named nothing AND nothing is
/// declared either, which is the same boundary in a worse state: unreported
/// as well as undeclared. Both exit non-zero, because a report that showed no
/// rows for a boundary nobody has declared would read as a clean fleet.
pub(crate) fn undeclared_row(
    declared: &[PublicOrigin],
    selection: &EdgeSelection,
) -> Option<Value> {
    let selected = match selection.origin.as_ref() {
        Some(selected) => selected.clone(),
        None if declared.is_empty() => String::new(),
        None => return None,
    };
    if declared.iter().any(|origin| origin.origin() == selected) {
        return None;
    }
    let named = !selected.is_empty();
    Some(json!({
        "schema": "stado.public-origin-report.v1",
        "name": Value::Null,
        "hostname": if named { selected.trim_start_matches("https://") } else { "" },
        "origin": if named { selected.as_str() } else { "" },
        "target": Value::Null,
        "publication": Value::Null,
        "upstream": Value::Null,
        "paths": [],
        "verdict": "origin-undeclared",
        "origin_error": if named {
            format!(
                "the public edge fetches release objects from {selected}, which no public_origins \
                 declaration names; declare it with `stado web origin declare`, or repoint the \
                 edge at an origin that is declared"
            )
        } else {
            format!(
                "nothing declares a public origin, and the public edge at {} could not be asked \
                 which origin it selected, so this boundary is neither declared nor reported: {}",
                selection.endpoint, selection.detail
            )
        },
        "resolution": {
            "state": ResolutionState::Unavailable.word(),
            "resolver": public_origin::resolve::PUBLIC_RESOLVER,
            "hostname": if named { selected.trim_start_matches("https://") } else { "" },
            "answers": [],
            "detail": "not asked: an origin nothing declares is repaired by declaring it, and resolving it would answer a question nobody has asked the fleet",
        },
        "publication_state": {
            "state": "unknown",
            "funnel_enabled": Value::Null,
            "port": Value::Null,
            "published_paths": [],
            "missing_paths": [],
            "undeclared_paths": [],
            "detail": "no declaration names a target for this origin, so there is no publication to read",
        },
        "edge_selection": selection.report(if named { "undeclared" } else { "unreadable" }),
    }))
}
