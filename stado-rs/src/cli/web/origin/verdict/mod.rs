//! The three readings a public-origin verdict is composed from, and the
//! precedence between them.
//!
//! Precedence is not cosmetic. A name that does not resolve makes every later
//! reading unactionable, so it is reported first; a target that could not be
//! reached is `unknown` and never `unpublished`, the same distinction
//! `stado storage stat` keeps between `absent` and `unavailable`; and an edge
//! that selected some other origin is the last thing worth saying, because
//! repointing an edge at a name that does not work would fix nothing.

mod edge;

use serde_json::{json, Value};

use super::report::declaration_row;
use crate::public_origin::{self, funnel, PublicOrigin, Resolution, ResolutionState};

pub(crate) use edge::{edge_selection, undeclared_row, EdgeSelection};

pub(crate) const VERDICT_SERVING: &str = "serving";

pub(crate) async fn examine(origin: &PublicOrigin, selection: &EdgeSelection) -> Value {
    let resolution = public_origin::resolve(&origin.hostname).await;
    let publication = publication_of(origin).await;
    let edge = edge::edge_state(origin, selection);
    let word = verdict_for(
        resolution.state,
        &publication,
        edge,
        selection.readback_answered(),
    );
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
            "readback": selection.readback,
        }),
    );
    row
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
    readback_answered: bool,
) -> &'static str {
    match resolution {
        ResolutionState::Unavailable => "resolver-unavailable",
        ResolutionState::Unresolved => "origin-not-public",
        ResolutionState::Resolved => match publication.state() {
            "unpublished" => "origin-unpublished",
            "unknown" => "origin-unreachable",
            _ if edge != "agrees" => "origin-mismatch",
            _ if !readback_answered => "origin-unreachable",
            _ => VERDICT_SERVING,
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
            "published" if edge == "agrees" && !selection.readback_answered() => {
                selection.readback_detail().to_string()
            }
            "published" => selection.detail.clone(),
            _ => publication.detail(),
        },
    }
}
