//! Finite authenticated Oko transcript-source operations on a selected host.
//!
//! This is a dispatch boundary, not a second importer. Oko owns discovery,
//! Transcript Lake adoption, catalog refresh, and their JSON receipts. Stado
//! owns resolving the declared host and carrying one fixed argv over its
//! authenticated host channel.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{HandlerError, HandlerResult};

const SUPPORTED_RUNTIMES: [&str; 5] = ["claude", "codex", "omp", "droid", "kimi"];
const OWNER_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcesRequest {
    host_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdoptRequest {
    host_id: String,
    runtime: String,
    root: String,
}

pub(super) fn supports(action: &str) -> bool {
    matches!(action, "transcript-sources" | "transcript-sources-adopt")
}

async fn target(host_id: &str) -> Result<crate::targets::ComputeTarget, HandlerError> {
    if host_id.is_empty() || host_id.trim() != host_id {
        return Err(HandlerError::BadRequest);
    }
    let registry = crate::deploy::host_channel::canonical_registry()
        .await
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    crate::deploy::host_channel::resolve_target(&registry, host_id)
        .cloned()
        .map_err(|_| HandlerError::BadRequest)
}

async fn run_owner(
    target: &crate::targets::ComputeTarget,
    program: &[&str],
) -> Result<Value, HandlerError> {
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_program(target, program, &runner)
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if !output.ok() || output.stdout.len() > OWNER_RESPONSE_LIMIT {
        return Err(HandlerError::UpstreamFailure);
    }
    serde_json::from_str(&output.stdout).map_err(|_| HandlerError::UpstreamFailure)
}

fn validate_sources(value: &Value) -> Result<(), HandlerError> {
    let entries = value.as_array().ok_or(HandlerError::UpstreamFailure)?;
    let mut runtimes = HashSet::with_capacity(entries.len());
    for entry in entries {
        let entry = entry.as_object().ok_or(HandlerError::UpstreamFailure)?;
        let runtime = entry
            .get("runtime")
            .and_then(Value::as_str)
            .ok_or(HandlerError::UpstreamFailure)?;
        if !SUPPORTED_RUNTIMES.contains(&runtime)
            || !runtimes.insert(runtime)
            || entry.get("available").and_then(Value::as_bool).is_none()
            || entry.get("mode").and_then(Value::as_str).is_none()
            || entry.get("selected").and_then(Value::as_bool).is_none()
            || entry.get("files").and_then(Value::as_u64).is_none()
            || entry
                .get("error")
                .is_some_and(|error| !error.is_null() && !error.is_string())
        {
            return Err(HandlerError::UpstreamFailure);
        }
        let roots = entry
            .get("roots")
            .and_then(Value::as_array)
            .ok_or(HandlerError::UpstreamFailure)?;
        if roots.iter().any(|root| {
            root.as_str().map_or(true, |root| {
                !Path::new(root).is_absolute() || root.contains('\0')
            })
        }) {
            return Err(HandlerError::UpstreamFailure);
        }
        let source_ids = entry
            .get("sourceIds")
            .and_then(Value::as_array)
            .ok_or(HandlerError::UpstreamFailure)?;
        if source_ids
            .iter()
            .any(|source_id| source_id.as_str().map_or(true, str::is_empty))
        {
            return Err(HandlerError::UpstreamFailure);
        }
    }
    Ok(())
}

async fn discover(host_id: &str) -> Result<(crate::targets::ComputeTarget, Value), HandlerError> {
    let target = target(host_id).await?;
    let sources = run_owner(&target, &["oko-cli", "transcripts", "sources", "--json"]).await?;
    validate_sources(&sources)?;
    Ok((target, sources))
}

fn discovered_root(sources: &Value, runtime: &str, root: &str) -> bool {
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

async fn sources(body: &[u8]) -> HandlerResult {
    let request: SourcesRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    let (_, sources) = discover(&request.host_id).await?;
    Ok(json!({"hostId": request.host_id, "sources": sources}))
}

async fn adopt(body: &[u8]) -> HandlerResult {
    let request: AdoptRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !SUPPORTED_RUNTIMES.contains(&request.runtime.as_str())
        || !Path::new(&request.root).is_absolute()
        || request.root.contains('\0')
    {
        return Err(HandlerError::BadRequest);
    }
    // Adoption is limited to a root the owner command discovered on this exact
    // host in this request. A caller cannot turn the finite action into an
    // arbitrary-path command runner.
    let (target, sources) = discover(&request.host_id).await?;
    if !discovered_root(&sources, &request.runtime, &request.root) {
        return Err(HandlerError::BadRequest);
    }
    let receipt = run_owner(
        &target,
        &[
            "oko-cli",
            "transcripts",
            "adopt",
            "--source",
            request.runtime.as_str(),
            "--root",
            request.root.as_str(),
            "--json",
        ],
    )
    .await?;
    let adoption = receipt
        .get("adoption")
        .and_then(Value::as_object)
        .ok_or(HandlerError::UpstreamFailure)?;
    let required_counts = [
        "candidateFiles",
        "candidateEvents",
        "pendingIncompleteLines",
        "imported",
        "unchanged",
        "conflicting",
        "rejected",
        "eventsImported",
    ];
    if adoption.get("runtime").and_then(Value::as_str) != Some(request.runtime.as_str())
        || adoption.get("root").and_then(Value::as_str) != Some(request.root.as_str())
        || adoption.get("selected").and_then(Value::as_bool) != Some(true)
        || adoption
            .get("status")
            .and_then(Value::as_str)
            .map_or(true, str::is_empty)
        || adoption
            .get("sourceId")
            .and_then(Value::as_str)
            .map_or(true, str::is_empty)
        || adoption
            .get("dataDir")
            .and_then(Value::as_str)
            .map_or(true, |path| !Path::new(path).is_absolute())
        || required_counts
            .iter()
            .any(|field| adoption.get(*field).and_then(Value::as_u64).is_none())
        || receipt
            .get("catalogProcessed")
            .and_then(Value::as_u64)
            .is_none()
        || receipt
            .get("database")
            .and_then(Value::as_str)
            .map_or(true, str::is_empty)
    {
        return Err(HandlerError::UpstreamFailure);
    }
    Ok(json!({"hostId": request.host_id, "result": receipt}))
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "transcript-sources" => sources(body).await,
        "transcript-sources-adopt" => adopt(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}
