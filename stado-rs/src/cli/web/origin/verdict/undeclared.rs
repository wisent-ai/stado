//! The report row for a public origin the edge selected that no declaration
//! names.

use serde_json::{json, Value};

use super::edge::EdgeSelection;
use crate::public_origin::{self, PublicOrigin, ResolutionState};

/// The row for a public origin no declaration covers.
///
/// Two shapes reach it. The edge named an origin nothing declares. Or the
/// edge named nothing AND nothing is declared either, which is the same
/// boundary in a worse state: unreported as well as undeclared. Both exit
/// non-zero, because a report that showed no rows for a boundary nobody has
/// declared would read as a clean fleet.
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
    let hostname = reqwest::Url::parse(&selected)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default();
    Some(json!({
        "schema": "stado.public-origin-report.v1",
        "name": Value::Null,
        "hostname": hostname,
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
            "hostname": hostname,
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
