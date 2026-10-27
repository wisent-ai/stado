//! What `oko transcripts sources --json` answers, read as the document Oko
//! writes. Every refused answer says which entry and which field refused it.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::super::HandlerError;

/// Oko owns which transcript runtimes exist; Stado only carries the call.
/// Stado used to keep its own five names here and fail the whole dashboard
/// response when Oko reported a source it had not heard of — which is what
/// happens the day Oko learns to read one more. The shape of a runtime
/// token is what this boundary can honestly check.
pub(super) fn is_runtime_token(runtime: &str) -> bool {
    !runtime.is_empty()
        && runtime
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// One transcript source as `oko transcripts sources --json` writes it. Oko
/// may add fields; the ones this boundary forwards must be present and typed.
#[derive(Deserialize)]
struct SourceEntry {
    runtime: String,
    #[allow(dead_code)]
    available: bool,
    #[allow(dead_code)]
    mode: String,
    #[allow(dead_code)]
    selected: bool,
    #[allow(dead_code)]
    files: u64,
    #[allow(dead_code)]
    error: Option<String>,
    roots: Vec<String>,
    #[serde(rename = "sourceIds")]
    source_ids: Vec<String>,
}

pub(super) fn validate_sources(value: &Value) -> Result<(), HandlerError> {
    let entries = Vec::<SourceEntry>::deserialize(value).map_err(|error| {
        HandlerError::upstream(format!(
            "oko transcripts sources answered a document this boundary cannot read ({error}): {value}"
        ))
    })?;
    let mut runtimes = HashSet::with_capacity(entries.len());
    for entry in &entries {
        if !is_runtime_token(&entry.runtime) {
            return Err(HandlerError::upstream(format!(
                "oko transcripts sources named a runtime that is not a token: {:?}",
                entry.runtime
            )));
        }
        if !runtimes.insert(entry.runtime.as_str()) {
            return Err(HandlerError::upstream(format!(
                "oko transcripts sources named the runtime {} twice",
                entry.runtime
            )));
        }
        if let Some(root) = entry
            .roots
            .iter()
            .find(|root| !Path::new(root).is_absolute() || root.contains('\0'))
        {
            return Err(HandlerError::upstream(format!(
                "oko transcripts sources gave runtime {} a root that is not an absolute path: {root:?}",
                entry.runtime
            )));
        }
        if entry.source_ids.iter().any(String::is_empty) {
            return Err(HandlerError::upstream(format!(
                "oko transcripts sources gave runtime {} an empty source id",
                entry.runtime
            )));
        }
    }
    Ok(())
}

/// Whether the discovery this host answered names `root` under `runtime`.
pub(super) fn discovered_root(sources: &Value, runtime: &str, root: &str) -> bool {
    sources.as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry.get("runtime").and_then(Value::as_str) == Some(runtime)
                && entry
                    .get("roots")
                    .and_then(Value::as_array)
                    .is_some_and(|roots| {
                        roots
                            .iter()
                            .any(|candidate| candidate.as_str() == Some(root))
                    })
        })
    })
}
