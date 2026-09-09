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
use crate::deploy::host_gates::{observe, ReadState};
use crate::deploy::DeployError;
use crate::targets::Registry;

pub(crate) use edge::{edge_selection, undeclared_row, EdgeSelection};

pub(crate) const VERDICT_SERVING: &str = "serving";

pub(crate) async fn examine(origin: &PublicOrigin, selection: &EdgeSelection, registry: &Registry) -> Value {
    let ((resolution, mut dns_read), (publication, publication_read)) = tokio::join!(
        observe("public_dns", format!("{}: {}", public_origin::resolve::PUBLIC_RESOLVER, origin.hostname),
            async { Ok::<_, DeployError>(public_origin::resolve(&origin.hostname).await) }),
        observe("publication", format!("{}: {} publication table", origin.target, origin.publication),
            publication_of(origin, registry)),
    );
    let resolution = resolution.unwrap_or_else(|| Resolution {
        state: ResolutionState::Unavailable, hostname: origin.hostname.clone(), answers: Vec::new(),
        detail: dns_read.detail.clone().unwrap_or_default(),
    });
    if resolution.state == ResolutionState::Unavailable && dns_read.complete() {
        dns_read.state = ReadState::Error;
        dns_read.detail = Some(resolution.detail.clone());
    }
    let publication = match publication {
        Some(publication) => PublicationReading::Read(publication),
        None => PublicationReading::Unknown(publication_read.detail.clone().unwrap_or_default()),
    };
    let complete = dns_read.complete() && publication_read.complete() && selection.observation.complete();
    let edge = edge::edge_state(origin, selection);
    let word = if complete {
        verdict_for(resolution.state, &publication, edge, selection.readback_answered())
    } else {
        "diagnostic-incomplete"
    };
    let problem = if complete {
        origin_error(&resolution, &publication, edge, selection)
    } else {
        [&dns_read, &publication_read, &selection.observation].into_iter()
            .filter_map(|read| read.detail.as_deref()).collect::<Vec<_>>().join("; ")
    };
    let mut row = declaration_row(origin);
    let object = row.as_object_mut().expect("a JSON object was just built");
    object.insert("schema".into(), json!("stado.public-origin-report.v1"));
    object.insert("complete".into(), json!(complete));
    object.insert("observations".into(), json!([dns_read, publication_read]));
    object.insert("verdict".into(), json!(word));
    object.insert("origin_error".into(), if word == VERDICT_SERVING { Value::Null } else { json!(problem) });
    object.insert("resolution".into(), resolution.to_json());
    object.insert("publication_state".into(), publication.to_json());
    object.insert("edge_selection".into(), selection.report(edge));
    row
}

/// Read the declared target's own publication table.
async fn publication_of(origin: &PublicOrigin, registry: &Registry) -> Result<funnel::Publication, DeployError> {
    let runner = crate::deploy::production_runner();
    let target = crate::deploy::host_channel::resolve_target(registry, &origin.target)?;
    funnel::read(origin, target, &runner).await.map_err(|error| DeployError(error.to_string()))
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
