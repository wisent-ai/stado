//! `stado resolver accept --generation N`: the operator's acceptance of one
//! directory generation the authority answers below the one this host's
//! resolver holds.
//!
//! A resolver refuses every generation below the one it loaded, so a host
//! that took a generation from a copy the authority never committed refuses
//! the authority forever. The rollback refusal names both store versions; the
//! operator who reads them and judges the held generation the false one says
//! so here, naming the exact generation the authority answers. The serving
//! resolver adopts that generation, and only that one, on its next refresh,
//! says so on stderr, and removes the acceptance, so it never accepts a later
//! rollback by itself. The acceptance is kept beside the resolver's published
//! state (`resolver-accept.json`).

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::cli::CmdError;

use super::report::published::{now_iso, state_path};

const ACCEPT_FILE: &str = "resolver-accept.json";

/// Where the acceptance is kept: beside the published resolver state.
fn path() -> Result<PathBuf, String> {
    let state = state_path()
        .ok_or_else(|| "this host has no home, so its resolver state has no place".to_string())?;
    let directory = state
        .parent()
        .ok_or_else(|| format!("the resolver state {} has no directory", state.display()))?;
    Ok(directory.join(ACCEPT_FILE))
}

/// Record the operator's acceptance of `generation`.
pub(super) fn accept(generation: u64, json_output: bool) -> Result<(), CmdError> {
    let path = path().map_err(CmdError::click)?;
    let record = json!({ "generation": generation, "acceptedAt": now_iso() });
    // Whole and flushed: the serving resolver reads it on its next refresh,
    // and half an acceptance read there was refused as unreadable.
    crate::primitives::document::persist(&path, record.to_string().as_bytes()).map_err(
        |error| {
            CmdError::click(format!(
                "the acceptance could not be written to {}: {error}",
                path.display()
            ))
        },
    )?;
    if json_output {
        println!("{}", json!({ "accepted": record, "path": path }));
    } else {
        println!(
            "accepted directory generation {generation}: this host's resolver adopts it on its next refresh \
             when the authority answers exactly that generation ({})",
            path.display()
        );
    }
    Ok(())
}

/// Whether the operator accepted exactly `generation`. An acceptance that
/// cannot be read is refused with its cause, never taken for none.
pub(in crate::cli::resolver) fn accepted(generation: u64) -> Result<bool, String> {
    let path = path()?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "the acceptance {} cannot be read: {error}",
                path.display()
            ))
        }
    };
    let record: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("the acceptance {} is not JSON: {error}", path.display()))?;
    let named = record["generation"].as_u64().ok_or_else(|| {
        format!(
            "the acceptance {} names no generation: {record}",
            path.display()
        )
    })?;
    Ok(named == generation)
}

/// Remove the acceptance once its generation is adopted, so no later
/// rollback is accepted by it.
pub(in crate::cli::resolver) fn consume() -> Result<(), String> {
    let path = path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "the used acceptance {} could not be removed: {error}",
            path.display()
        )),
    }
}
