//! The three readings a public-origin verdict is composed from, and the
//! precedence between them.
//!
//! Precedence is not cosmetic. A name that does not resolve makes every later
//! reading unactionable, so it is reported first; a target that could not be
//! reached is `unknown` and never `unpublished`, the same distinction
//! `stado storage stat` keeps between `absent` and `unavailable`; and an edge
//! that selected some other origin is the last thing worth saying, because
//! repointing an edge at a name that does not work would fix nothing.

use serde_json::{json, Value};

use super::report::declaration_row;
use crate::public_origin::{self, funnel, PublicOrigin, Resolution, ResolutionState};

/// Where the public edge reports the origin it selected. The path is the
/// release route's sibling: one route serves bytes, this one serves the
/// selection, and the second exists because the first only named its origin
/// while failing.
const SELECTION_PATH: &str = "/api/release/origin";

pub(crate) const VERDICT_SERVING: &str = "serving";

/// What the live public edge says it selected.
pub(crate) struct EdgeSelection {
    pub endpoint: String,
    pub origin: Option<String>,
    pub detail: String,
}

pub(crate) async fn edge_selection() -> EdgeSelection {
    let endpoint = format!(
        "{}{SELECTION_PATH}",
        crate::config::stado_api_url().trim_end_matches('/')
    );
    let client = match crate::cli::storage::fleet_https_client() {
        Ok(client) => client,
        Err(error) => {
            return EdgeSelection {
                endpoint,
                origin: None,
                detail: format!("this Stado could not build its HTTPS client: {error}"),
            }
        }
    };
    let response = match client.get(&endpoint).send().await {
        Ok(response) => response,
        Err(error) => {
            return EdgeSelection {
                endpoint,
                origin: None,
                detail: format!("the public edge did not answer: {error}"),
            }
        }
    };
    let status = response.status().as_u16();
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => format!("the response body could not be read: {error}"),
    };
    let selected = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value["origin"].as_str().map(str::to_string));
    match selected {
        Some(origin) => EdgeSelection {
            endpoint,
            detail: format!("the public edge reports it fetches release objects from {origin}"),
            origin: Some(origin),
        },
        None => EdgeSelection {
            endpoint,
            origin: None,
            detail: format!(
                "the public edge answered HTTP {status} and named no selected origin: {}",
                quoted_body(&body)
            ),
        },
    }
}

/// How much of an unexpected response body a receipt carries.
///
/// The answer to this request is a small JSON document. A deployment that
/// predates the selection route answers with the site's own 404 page instead,
/// and pasting a Next.js document into a receipt buries the one sentence an
/// operator needs under sixty kilobytes of script tags.
const BODY_EXCERPT_BYTES: usize = 200;

fn quoted_body(body: &str) -> String {
    let body = body.trim();
    let head: String = body.chars().take(BODY_EXCERPT_BYTES).collect();
    if head.len() == body.len() {
        return head;
    }
    format!("{head}… ({} bytes in total, excerpted)", body.len())
}

pub(crate) async fn examine(origin: &PublicOrigin, selection: &EdgeSelection) -> Value {
    let resolution = public_origin::resolve(&origin.hostname).await;
    let publication = publication_of(origin).await;
    let edge = edge_state(origin, selection);
    let word = verdict_for(resolution.state, &publication, edge);
    let mut row = declaration_row(origin);
    let object = row.as_object_mut().expect("a JSON object was just built");
    object.insert("schema".into(), json!("stado.public-origin-report.v1"));
    object.insert("verdict".into(), json!(word));
    object.insert(
        "origin_error".into(),
        if word == VERDICT_SERVING {
            Value::Null
        } else {
            json!(origin_error(&resolution, &publication, edge, selection))
        },
    );
    object.insert("resolution".into(), resolution.to_json());
    object.insert("publication_state".into(), publication.to_json());
    object.insert(
        "edge_selection".into(),
        json!({
            "state": edge,
            "origin": selection.origin,
            "endpoint": selection.endpoint,
            "detail": selection.detail,
        }),
    );
    row
}

fn edge_state(origin: &PublicOrigin, selection: &EdgeSelection) -> &'static str {
    match &selection.origin {
        Some(selected) if *selected == origin.origin() => "agrees",
        Some(_) => "differs",
        None => "unreadable",
    }
}

/// Read the declared target's own publication table.
async fn publication_of(origin: &PublicOrigin) -> PublicationReading {
    let runner = crate::deploy::production_runner();
    let target = match crate::deploy::host_channel::canonical_target(&origin.target).await {
        Ok(target) => target,
        Err(error) => {
            return PublicationReading::Unknown(format!(
                "the registry could not resolve target {}: {error}",
                origin.target
            ))
        }
    };
    match funnel::read(origin, &target, &runner).await {
        Ok(publication) => PublicationReading::Read(publication),
        Err(error) => PublicationReading::Unknown(error.0),
    }
}

pub(crate) enum PublicationReading {
    Read(funnel::Publication),
    Unknown(String),
}

impl PublicationReading {
    pub fn state(&self) -> &'static str {
        match self {
            Self::Read(publication) => publication.state(),
            Self::Unknown(_) => "unknown",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::Read(publication) if publication.state() == "published" => {
                "the declared target publishes every declared path".to_string()
            }
            Self::Read(publication) => format!(
                "funnel_enabled={}, not published as declared: {}",
                publication.funnel_enabled,
                publication.missing.join(", ")
            ),
            Self::Unknown(detail) => detail.clone(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut value = match self {
            Self::Read(publication) => publication.to_json(),
            Self::Unknown(_) => json!({
                "state": "unknown",
                "funnel_enabled": Value::Null,
                "port": Value::Null,
                "published_paths": [],
                "missing_paths": [],
                "undeclared_paths": [],
            }),
        };
        value
            .as_object_mut()
            .expect("a publication reading is an object")
            .insert("detail".into(), json!(self.detail()));
        value
    }
}

fn verdict_for(
    resolution: ResolutionState,
    publication: &PublicationReading,
    edge: &'static str,
) -> &'static str {
    match resolution {
        ResolutionState::Unavailable => "resolver-unavailable",
        ResolutionState::Unresolved => "origin-not-public",
        ResolutionState::Resolved => match publication.state() {
            "unpublished" => "origin-unpublished",
            "unknown" => "origin-unreachable",
            _ if edge == "agrees" => VERDICT_SERVING,
            _ => "origin-mismatch",
        },
    }
}

fn origin_error(
    resolution: &Resolution,
    publication: &PublicationReading,
    edge: &'static str,
    selection: &EdgeSelection,
) -> String {
    match resolution.state {
        ResolutionState::Unavailable | ResolutionState::Unresolved => resolution.detail.clone(),
        ResolutionState::Resolved => match publication.state() {
            "published" if edge == "differs" => format!(
                "{} publishes every declared path, but {}",
                resolution.hostname, selection.detail
            ),
            "published" => selection.detail.clone(),
            _ => publication.detail(),
        },
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
    let selected = &selected;
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
            "hostname": selected.trim_start_matches("https://"),
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
        "edge_selection": {
            "state": if named { "undeclared" } else { "unreadable" },
            "origin": selection.origin,
            "endpoint": selection.endpoint,
            "detail": selection.detail,
        },
    }))
}
