//! `stado registry push` — upload an operator's document, and the typed
//! receipt that reports every outcome including the refusal.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::registry::commands::source_path;
use crate::cli::registry::write::conflict::RegistryWriteError;
use crate::cli::registry::write::document::{validate_for_write, warn_scoped_validation};
use crate::cli::registry::write::upload::upload_payload;
use crate::cli::CmdError;
use crate::targets;

/// `stado registry push --json`, for every outcome including the refusal.
///
/// A caller that has to scrape "pushed ... generation=..." out of a sentence
/// to learn whether its edit landed is a caller that will one day mistake a
/// refusal for a success. `expected_generation` is the caller's own token or
/// null; `actual_generation` is what the object carried instead and is only
/// ever set on a `conflict`; `generation` and `replaced` are only ever set on
/// a `pushed`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryPushReceipt {
    schema: String,
    state: String,
    location: String,
    expected_generation: Option<String>,
    actual_generation: Option<String>,
    generation: Option<String>,
    replaced: Option<String>,
}

/// `stado registry push PATH|- [--if-generation TOKEN] [--force]
/// [--allow-empty-fleet] [--json]` — upload an operator's document.
///
/// `if_generation` is the token an earlier [`pull`](crate::cli::registry::pull) handed back. With it the
/// write is conditional on the read the edit was made against, so a
/// concurrent publication is refused with [`REGISTRY_CONFLICT_EXIT`](crate::cli::registry::REGISTRY_CONFLICT_EXIT) instead
/// of overwritten; without it the write is only conditional on the read
/// [`upload_payload`] does itself, which is what every push did before the
/// flag existed.
pub async fn push(
    path: Option<String>,
    force: bool,
    allow_empty_fleet: bool,
    if_generation: Option<String>,
    json_output: bool,
) -> Result<(), CmdError> {
    // This command takes a PATH, and with no path it falls back to the
    // repository's bundled document. A caller who pipes a body is therefore
    // silently ignored and something else is uploaded in its place - which is
    // exactly how the bundled 65-byte skeleton reached the canonical registry
    // on 2026-09-01. Refuse the ambiguity rather than resolve it silently,
    // and let `-` mean stdin for a caller who meant to pipe.
    let from_stdin = path.as_deref() == Some("-");
    if !from_stdin && path.is_none() && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(CmdError::usage(
            "a document was piped to `registry push` but this command reads a PATH, so the \
             piped bytes would be ignored and the bundled registry uploaded instead. Pass the \
             file's path, or `-` to read stdin deliberately.",
        ));
    }
    let (source, payload) = if from_stdin {
        let mut body = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut body)?;
        (PathBuf::from("<stdin>"), body)
    } else {
        let source = source_path(path);
        let payload = std::fs::read_to_string(&source)?;
        (source, payload)
    };
    let document: Value = serde_json::from_str(&payload)
        .map_err(|exc| CmdError::click(format!("{}: {exc}", source.display())))?;
    // Ahead of every store call, as it has always been: a document that would
    // not validate never reaches the registry, whatever token it carries.
    warn_scoped_validation(validate_for_write(&document).await?);
    let location = targets::registry_location();
    match upload_payload(&payload, force, allow_empty_fleet, if_generation.as_deref()).await {
        Ok((generation, previous_generation)) => {
            if json_output {
                return print_push_receipt(&RegistryPushReceipt {
                    schema: PUSH_RECEIPT_SCHEMA.to_string(),
                    state: "pushed".to_string(),
                    location,
                    expected_generation: if_generation,
                    actual_generation: None,
                    generation: Some(generation),
                    replaced: Some(previous_generation),
                });
            }
            println!(
                "pushed {} -> {location} generation={generation} replaced={previous_generation}",
                source.display()
            );
            Ok(())
        }
        Err(RegistryWriteError::Conflict(conflict)) => {
            // The receipt goes out before the error, so a `--json` caller has
            // the two generations in hand no matter how it treats exit 75.
            if json_output {
                print_push_receipt(&RegistryPushReceipt {
                    schema: PUSH_RECEIPT_SCHEMA.to_string(),
                    state: "conflict".to_string(),
                    location: conflict.location.clone(),
                    expected_generation: Some(conflict.expected.clone()),
                    actual_generation: conflict.actual_generation().map(str::to_string),
                    generation: None,
                    replaced: None,
                })?;
            }
            Err(conflict.error())
        }
        // A storage or verification failure is not a conflict: nothing about
        // it says "re-read and re-apply", so it keeps exit 1 and emits no
        // receipt for a caller to mistake for a decision about generations.
        Err(RegistryWriteError::Failed(error)) => Err(error),
    }
}

const PUSH_RECEIPT_SCHEMA: &str = "stado.registry-push-receipt.v1";

fn print_push_receipt(receipt: &RegistryPushReceipt) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(receipt)?);
    Ok(())
}
