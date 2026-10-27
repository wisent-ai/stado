//! Finite authenticated Oko transcript-source operations on a selected host.
//!
//! This is a dispatch boundary, not a second importer. Oko owns discovery,
//! Transcript Lake adoption, catalog refresh, and their JSON receipts. Stado
//! owns resolving the declared host and carrying one fixed argv over its
//! authenticated host channel.

mod sources;

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use self::sources::{discovered_root, is_runtime_token, validate_sources};
use super::oko_automation::OKO;
use super::{HandlerError, HandlerResult};

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
        || super::oko_automation::supports(action)
}

/// A registry that could not be read, logged like an upstream failure: the
/// caller still gets `provider_unavailable`, the log says which read failed.
fn unavailable<E: std::fmt::Display>(error: E) -> HandlerError {
    tracing::warn!(cause = %error, "integration host registry read failed");
    HandlerError::ProviderUnavailable
}

/// A host the registry does not declare, logged with the registry's own
/// refusal so a mistyped host id and an unregistered host can be told apart.
fn unknown_host<E: std::fmt::Display>(host_id: &str, error: E) -> HandlerError {
    tracing::warn!(host_id, cause = %error, "integration host is not declared");
    HandlerError::BadRequest
}

pub(super) async fn target(host_id: &str) -> Result<crate::targets::ComputeTarget, HandlerError> {
    if host_id.is_empty() || host_id.trim() != host_id {
        return Err(HandlerError::BadRequest);
    }
    let registry = crate::deploy::host_channel::canonical_registry()
        .await
        .map_err(unavailable)?;
    crate::deploy::host_channel::resolve_target(&registry, host_id)
        .cloned()
        .map_err(|error| unknown_host(host_id, error))
}

async fn run_owner(
    target: &crate::targets::ComputeTarget,
    program: &[&str],
) -> Result<Value, HandlerError> {
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_program(target, program, &runner)
        .await
        .map_err(HandlerError::upstream)?;
    if !output.succeeded() {
        return Err(HandlerError::upstream(format!(
            "{} exited {:?} on the Oko host: {}",
            program.join(" "),
            output.code,
            output.stderr
        )));
    }
    serde_json::from_str(&output.stdout).map_err(|error| {
        HandlerError::upstream(format!(
            "{} answered output that is not JSON ({error}): {}",
            program.join(" "),
            output.stdout
        ))
    })
}

async fn discover(host_id: &str) -> Result<(crate::targets::ComputeTarget, Value), HandlerError> {
    let target = target(host_id).await?;
    let sources = run_owner(&target, &[OKO, "transcripts", "sources", "--json"]).await?;
    validate_sources(&sources)?;
    Ok((target, sources))
}

/// A request body this boundary cannot read, logged with serde's reason so a
/// caller sending an old field name can be told from one sending no JSON.
fn unreadable_request<E: std::fmt::Display>(action: &str, error: E) -> HandlerError {
    tracing::warn!(action, cause = %error, "integration request body unreadable");
    HandlerError::BadRequest
}

async fn sources(body: &[u8]) -> HandlerResult {
    let request: SourcesRequest = serde_json::from_slice(body)
        .map_err(|error| unreadable_request("transcript-sources", error))?;
    let (_, sources) = discover(&request.host_id).await?;
    Ok(json!({"hostId": request.host_id, "sources": sources}))
}

async fn adopt(body: &[u8]) -> HandlerResult {
    let request: AdoptRequest = serde_json::from_slice(body)
        .map_err(|error| unreadable_request("transcript-sources-adopt", error))?;
    // The runtime is not checked against a list here: the discovery below
    // is the authority, and it refuses a runtime/root pair this host did
    // not report.
    if !is_runtime_token(&request.runtime)
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
            OKO,
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
    // The receipt's shape, as a type rather than a list of field names:
    // serde refuses a missing or wrongly typed count, and the eight counts
    // Oko reports cannot drift from the eight this boundary checks.
    #[derive(Deserialize)]
    struct AdoptionReceipt {
        runtime: String,
        root: String,
        selected: bool,
        status: String,
        #[serde(rename = "sourceId")]
        source_id: String,
        #[serde(rename = "dataDir")]
        data_dir: String,
        #[serde(rename = "candidateFiles")]
        _candidate_files: u64,
        #[serde(rename = "candidateEvents")]
        _candidate_events: u64,
        #[serde(rename = "pendingIncompleteLines")]
        _pending_incomplete_lines: u64,
        _imported: u64,
        _unchanged: u64,
        _conflicting: u64,
        _rejected: u64,
        #[serde(rename = "eventsImported")]
        _events_imported: u64,
    }
    #[derive(Deserialize)]
    struct CatalogueReceipt {
        #[serde(rename = "catalogProcessed")]
        _processed: u64,
        database: String,
    }
    // A receipt that lacks the adoption block, or whose block does not fit
    // the type, is an upstream failure whose cause the log carries.
    let adoption = match receipt.get("adoption") {
        Some(block) => {
            serde_json::from_value::<AdoptionReceipt>(block.clone()).map_err(|error| {
                HandlerError::upstream(format!(
                    "oko transcripts adopt: adoption block unreadable: {error}"
                ))
            })?
        }
        None => {
            return Err(HandlerError::upstream(
                "oko transcripts adopt answered no adoption block",
            ));
        }
    };
    let catalogue =
        serde_json::from_value::<CatalogueReceipt>(receipt.clone()).map_err(|error| {
            HandlerError::upstream(format!(
                "oko transcripts adopt: catalogue receipt unreadable: {error}"
            ))
        })?;
    let disagreement = if adoption.runtime != request.runtime {
        Some(format!(
            "runtime {} is not the requested {}",
            adoption.runtime, request.runtime
        ))
    } else if adoption.root != request.root {
        Some(format!(
            "root {} is not the requested {}",
            adoption.root, request.root
        ))
    } else if !adoption.selected {
        Some("the adopted source is not selected".to_string())
    } else if adoption.status.is_empty() {
        Some("the adoption status is empty".to_string())
    } else if adoption.source_id.is_empty() {
        Some("the source id is empty".to_string())
    } else if !Path::new(&adoption.data_dir).is_absolute() {
        Some(format!(
            "data dir {} is not an absolute path",
            adoption.data_dir
        ))
    } else if catalogue.database.is_empty() {
        Some("the catalogue names no database".to_string())
    } else {
        None
    };
    if let Some(cause) = disagreement {
        return Err(HandlerError::upstream(format!(
            "oko transcripts adopt answered a receipt that does not match the request: {cause}"
        )));
    }
    Ok(json!({"hostId": request.host_id, "result": receipt}))
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    if super::oko_automation::supports(action) {
        return super::oko_automation::handle(action, body).await;
    }
    match action {
        "transcript-sources" => sources(body).await,
        "transcript-sources-adopt" => adopt(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}
